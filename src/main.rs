use colink::{CoLink, Participant, ProtocolEntry};
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    net::TcpListener,
    os::unix::prelude::OwnedFd,
    process::{Command, Stdio},
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
            let fd = OwnedFd::from(stream?);
            let dir = if std::env::var("HOME").is_ok() {
                std::env::var("HOME").unwrap()
            } else {
                "/".to_string()
            };
            let shell_process = Command::new("bash")
                .arg("-i")
                .current_dir(dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::from(fd.try_clone()?))
                .stderr(Stdio::from(fd.try_clone()?))
                .spawn()?;

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

            let mut stdin = shell_process.stdin.as_ref().unwrap();
            let mut output = File::from(fd);
            let mut id = 0;
            loop {
                let cmd = cl
                    .get_variable(&format!("command:{}", id), &participants[0])
                    .await?;
                if cmd.is_empty() {
                    break;
                }
                if self.tg_approval(&cl, &participants, &cmd).await? {
                    stdin.write_all(&cmd)?;
                } else {
                    output.write_all("Rejected, enter the new command: ".as_bytes())?;
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
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let mut params = HashMap::new();
        let text = format!(
            "User {} want to run the following command:\n{}",
            participants[1].user_id,
            String::from_utf8_lossy(cmd)
        );
        params.insert("text", text);
        let callback_token = uuid::Uuid::new_v4().to_string();
        let mut inline_keyboard_entries: Vec<Vec<HashMap<&str, String>>> = vec![];
        let mut inline_keyboard_entry: Vec<HashMap<&str, String>> = vec![];
        for entry in ["Approve", "Reject", "Ignore"] {
            let mut map: HashMap<&str, String> = HashMap::new();
            map.insert("text", entry.to_string());
            map.insert("callback_data", format!("1 {} {}", callback_token, entry));
            inline_keyboard_entry.push(map);
        }
        inline_keyboard_entries.push(inline_keyboard_entry);
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
        let action = String::from_utf8_lossy(&action);
        if action == "Approve" {
            Ok(true)
        } else if action == "Reject" {
            Ok(false)
        } else {
            thread::sleep(core::time::Duration::MAX);
            Ok(false)
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
