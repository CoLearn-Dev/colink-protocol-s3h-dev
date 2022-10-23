use colink::{CoLink, Participant, ProtocolEntry};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpListener,
    os::unix::{net::UnixStream, prelude::OwnedFd},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

struct Server;
#[colink::async_trait]
impl ProtocolEntry for Server {
    async fn start(
        &self,
        cl: CoLink,
        _param: Vec<u8>,
        participants: Vec<Participant>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        let addr = "127.0.0.1:8088";
        let listener = TcpListener::bind(addr)?;
        cl.set_variable("screen", addr.as_bytes(), &[participants[0].clone()])
            .await?;
        if let Some(stream) = listener.incoming().next() {
            let mut stream = stream?;
            let (sock1, mut sock2) = UnixStream::pair().unwrap();
            let dir = if std::env::var("HOME").is_ok() {
                std::env::var("HOME").unwrap()
            } else {
                "/".to_string()
            };
            let fd = OwnedFd::from(sock1);
            let shell_process = Command::new("bash")
                .arg("-i")
                .current_dir(dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::from(fd.try_clone()?))
                .stderr(Stdio::from(fd.try_clone()?))
                .spawn()?;

            // ctrlc
            let shell_process_pid = shell_process.id();
            let cl_clone = cl.clone();
            let p0 = participants[0].clone();
            tokio::spawn(async move {
                let mut id = 0;
                loop {
                    let _ctrlc = cl_clone.get_variable(&format!("ctrlc:{}", id), &p0).await?;
                    Command::new("pkill")
                        .arg("-INT")
                        .arg("-P")
                        .arg(&shell_process_pid.to_string())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()?;
                    id += 1;
                }
                #[allow(unreachable_code)]
                Ok::<(), Box<dyn std::error::Error + Send + Sync + 'static>>(())
            });

            // output
            let enable_monitor = Arc::new(Mutex::new(false));
            let enable_monitor_clone = enable_monitor.clone();
            let cl_clone = cl.clone();
            let mut stream_clone = stream.try_clone()?;
            tokio::spawn(async move {
                loop {
                    let mut buffer = [0; 4096];
                    loop {
                        let nbytes = sock2.read(&mut buffer).unwrap();
                        if nbytes == 0 {
                            thread::sleep(core::time::Duration::from_millis(10));
                            continue;
                        }
                        stream_clone.write_all(&buffer[..nbytes]).unwrap();
                        let enable_monitor = *enable_monitor_clone.lock().unwrap();
                        if enable_monitor {
                            cl_clone
                                .run_task(
                                    "telegram_bot.send_msg",
                                    &buffer[..nbytes],
                                    &[Participant {
                                        user_id: cl_clone.get_user_id()?,
                                        role: "default".to_string(),
                                    }],
                                    false,
                                )
                                .await?;
                        }
                    }
                }
                #[allow(unreachable_code)]
                Ok::<(), Box<dyn std::error::Error + Send + Sync + 'static>>(())
            });

            // incoming command
            let mut stdin = shell_process.stdin.as_ref().unwrap();
            let mut id = 0;
            loop {
                let cmd = cl
                    .get_variable(&format!("command:{}", id), &participants[0])
                    .await?;
                if cmd.is_empty() {
                    *enable_monitor.lock().unwrap() = false;
                    break;
                }
                let decision = self.tg_approval(&cl, &participants, &cmd).await?;
                if decision <= 1 {
                    if decision == 1 {
                        // enable_monitor
                        *enable_monitor.lock().unwrap() = true;
                    } else {
                        *enable_monitor.lock().unwrap() = false;
                    }
                    stdin.write_all(&cmd)?;
                } else {
                    stream.write_all("Rejected, enter the new command: ".as_bytes())?;
                }
                id += 1;
            }
            println!("exit!");
        }
        Ok(())
    }
}

impl Server {
    async fn tg_approval(
        &self,
        cl: &CoLink,
        participants: &[Participant],
        cmd: &[u8],
    ) -> Result<i32, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let mut params = HashMap::new();
        let text = format!(
            "User {} want to run the following command:\n{}",
            participants[1].user_id,
            String::from_utf8_lossy(cmd)
        );
        params.insert("text", text);
        let callback_token = uuid::Uuid::new_v4().to_string();
        let mut inline_keyboard_entries: Vec<Vec<HashMap<&str, String>>> = vec![];
        for entries in [["Approve", "Reject"], ["Approve and Monitor", "Ignore"]] {
            let mut inline_keyboard_entry: Vec<HashMap<&str, String>> = vec![];
            for entry in entries {
                let mut map: HashMap<&str, String> = HashMap::new();
                map.insert("text", entry.to_string());
                map.insert("callback_data", format!("1 {} {}", callback_token, entry));
                inline_keyboard_entry.push(map);
            }
            inline_keyboard_entries.push(inline_keyboard_entry);
        }
        let reply_markup = format!(
            "{{\"inline_keyboard\":{}}}",
            serde_json::to_string(&inline_keyboard_entries)?
        );
        params.insert("reply_markup", reply_markup);
        cl.run_task(
            "telegram_bot.send_msg_with_reply_markup",
            &serde_json::to_vec(&params)?,
            &[Participant {
                user_id: cl.get_user_id()?,
                role: "default".to_string(),
            }],
            false,
        )
        .await?;
        let action = cl
            .read_or_wait(&format!("tg_bot:callback:{}", callback_token))
            .await?;
        let decision = String::from_utf8_lossy(&action);
        if decision == "Approve" {
            Ok(0)
        } else if decision == "Approve and Monitor" {
            Ok(1)
        } else if decision == "Reject" {
            Ok(2)
        } else {
            thread::sleep(core::time::Duration::MAX);
            Ok(3)
        }
    }
}

struct Client;
#[colink::async_trait]
impl ProtocolEntry for Client {
    async fn start(
        &self,
        _cl: CoLink,
        _param: Vec<u8>,
        _participants: Vec<Participant>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        Ok(())
    }
}

colink::protocol_start!(
    ("remote_shell:server", Server),
    ("remote_shell:client", Client)
);