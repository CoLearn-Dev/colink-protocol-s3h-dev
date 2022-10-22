use colink::{CoLink, Participant, ProtocolEntry};
use std::{
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
                .stderr(Stdio::from(fd))
                .spawn()?;
            let mut stdin = shell_process.stdin.as_ref().unwrap();
            let mut id = 0;
            loop {
                let cmd = cl
                    .get_variable(&format!("command:{}", id), &participants[0])
                    .await?;
                if cmd.len() == 0 {
                    break;
                }
                // TODO approval
                thread::sleep(core::time::Duration::from_millis(100));
                // approval
                stdin.write_all(&cmd)?;
                id += 1;
            }
            println!("exit!");
        }
        Ok(())
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
