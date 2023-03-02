#![allow(clippy::uninlined_format_args)]
use colink::{CoLink, Participant, ProtocolEntry};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    os::unix::{net::UnixStream, prelude::OwnedFd},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicI32, Ordering},
        Arc, Mutex,
    },
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
        cl.send_variable("start_session", b"", &[participants[0].clone()])
            .await?;
        let (sock1, mut sock2) = UnixStream::pair().unwrap();
        let dir = if std::env::var("HOME").is_ok() {
            std::env::var("HOME").unwrap()
        } else {
            "/".to_string()
        };
        let fd = OwnedFd::from(sock1);
        let shell_process = Command::new("bash")
            .arg("-i")
            .env("TERM", "xterm-256color")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(fd.try_clone()?))
            .stderr(Stdio::from(fd.try_clone()?))
            .spawn()?;

        // ctrlc
        let shell_process_pid = shell_process.id();
        let cl_clone = cl.clone();
        let p0 = participants[0].clone();
        thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let mut id = 0;
                    loop {
                        let _ctrlc = cl_clone
                            .recv_variable(&format!("ctrlc:{}", id), &p0)
                            .await?;
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
                })?;
            Ok::<(), Box<dyn std::error::Error + Send + Sync + 'static>>(())
        });

        // output
        let enable_monitor = Arc::new(Mutex::new(false));
        let enable_monitor_clone = enable_monitor.clone();
        let cl_clone = cl.clone();
        let p0 = participants[0].clone();
        let output_id = Arc::new(AtomicI32::new(0));
        let output_id_clone = output_id.clone();
        thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    loop {
                        let mut buffer = [0; 4096];
                        loop {
                            let nbytes = sock2.read(&mut buffer).unwrap();
                            if nbytes == 0 {
                                tokio::time::sleep(core::time::Duration::from_millis(10)).await;
                                continue;
                            }

                            let id = output_id_clone.fetch_add(1, Ordering::Relaxed);
                            cl_clone
                                .send_variable(
                                    &format!("output:{}", id),
                                    &buffer[..nbytes],
                                    &[p0.clone()],
                                )
                                .await?;

                            let enable_monitor = *enable_monitor_clone.lock().unwrap();
                            if enable_monitor {
                                let mut params = HashMap::new();
                                params.insert(
                                    "text",
                                    format!(
                                        "```\n{}\n```",
                                        markdown_escape(ansi_clean_up(&buffer[..nbytes]))
                                    ),
                                );
                                params.insert("parse_mode", "MarkdownV2".to_string());
                                cl_clone
                                    .run_task(
                                        "telegram_bot.send_msg_with_parse_mode",
                                        &serde_json::to_vec(&params)?,
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
                })?;
            Ok::<(), Box<dyn std::error::Error + Send + Sync + 'static>>(())
        });
        // incoming command
        let mut preapproved_cmds = HashSet::new();
        let mut stdin = shell_process.stdin.as_ref().unwrap();
        let mut id = 0;
        loop {
            let cmd = cl
                .recv_variable(&format!("command:{}", id), &participants[0])
                .await?;
            if cmd.is_empty() {
                *enable_monitor.lock().unwrap() = false;
                break;
            }
            let decision = self
                .tg_approval(&cl, &participants, &cmd, &mut preapproved_cmds)
                .await?;
            if decision <= 1 {
                if decision == 1 {
                    *enable_monitor.lock().unwrap() = true;
                } else {
                    *enable_monitor.lock().unwrap() = false;
                }
                stdin.write_all(&cmd)?;
            } else {
                let id = output_id.fetch_add(1, Ordering::Relaxed);
                cl.send_variable(
                    &format!("output:{}", id),
                    b"Rejected, enter a new command: ",
                    &[participants[0].clone()],
                )
                .await?;
            }
            id += 1;
        }
        println!("exit!");
        Ok(())
    }
}

impl Server {
    async fn tg_approval(
        &self,
        cl: &CoLink,
        participants: &[Participant],
        cmd: &[u8],
        preapproved_cmds: &mut HashSet<String>,
    ) -> Result<i32, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let cmd = String::from_utf8_lossy(cmd).to_string();
        if preapproved_cmds.contains(&cmd) {
            return Ok(0);
        }
        let mut params = HashMap::new();
        let text = format!(
            "User {} want to run the following command:\n{}",
            &participants[0].user_id[..7],
            cmd
        );
        params.insert("text", text);
        let callback_token = uuid::Uuid::new_v4().to_string();
        let mut inline_keyboard_entries: Vec<Vec<HashMap<&str, String>>> = vec![];
        for entries in [
            vec![("Approve", "Approve"), ("Reject", "Reject")],
            vec![
                ("Approve and monitor", "Approve and monitor"),
                ("Ignore", "Ignore"),
            ],
            vec![("Approve all the same commands", "Approve_all")],
        ] {
            let mut inline_keyboard_entry: Vec<HashMap<&str, String>> = vec![];
            for (text, callback_data) in entries {
                let mut map: HashMap<&str, String> = HashMap::new();
                map.insert("text", text.to_string());
                map.insert(
                    "callback_data",
                    format!("1 {} {}", callback_token, callback_data),
                );
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
        if !cmd.is_empty() && decision == "Approve_all" {
            preapproved_cmds.insert(cmd);
        }
        if decision == "Approve" || decision == "Approve_all" {
            Ok(0)
        } else if decision == "Approve and monitor" {
            Ok(1)
        } else if decision == "Reject" {
            Ok(2)
        } else {
            tokio::time::sleep(core::time::Duration::MAX).await;
            Ok(3)
        }
    }
}

fn ansi_clean_up(text: &[u8]) -> String {
    let text = String::from_utf8_lossy(text);
    let text = regex::Regex::new(r"\x1b\[[0-9;]*m")
        .unwrap()
        .replace_all(&text, "");
    let text = regex::Regex::new(r"\x1b\]0;(.*?)(\n|$)")
        .unwrap()
        .replace_all(&text, "");
    let text = regex::Regex::new(r"\x1b\]0;(.*?)\a")
        .unwrap()
        .replace_all(&text, "");
    text.to_string()
}

fn markdown_escape(text: String) -> String {
    let text_escaped: String = text
        .chars()
        .map(|x| match x {
            '\\' => "\\\\".to_string(),
            '_' => "\\_".to_string(),
            '*' => "\\*".to_string(),
            '[' => "\\[".to_string(),
            ']' => "\\]".to_string(),
            '(' => "\\(".to_string(),
            ')' => "\\)".to_string(),
            '~' => "\\~".to_string(),
            '`' => "\\`".to_string(),
            '>' => "\\>".to_string(),
            '#' => "\\#".to_string(),
            '+' => "\\+".to_string(),
            '-' => "\\-".to_string(),
            '=' => "\\=".to_string(),
            '|' => "\\|".to_string(),
            '{' => "\\{".to_string(),
            '}' => "\\}".to_string(),
            '.' => "\\.".to_string(),
            '!' => "\\!".to_string(),
            _ => x.to_string(),
        })
        .collect();
    text_escaped
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

colink::protocol_start!(("s3h:server", Server), ("s3h:client", Client));
