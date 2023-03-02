use colink::{CoLink, Participant};
use std::{
    cmp::min,
    io::{self, BufRead, Write},
    sync::{mpsc::channel, Arc, Mutex},
    thread,
};

pub(crate) async fn s3h_session(
    cl: &CoLink,
    participants: &[Participant],
) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    // output
    let waiting_for_approval = Arc::new(Mutex::new(false));
    let waiting_for_approval_clone = waiting_for_approval.clone();
    let last_cmd: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let last_cmd_clone = last_cmd.clone();
    let cl_clone = cl.clone();
    let p1 = participants[1].clone();
    thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let mut id = 0;
                loop {
                    let buffer = cl_clone
                        .recv_variable(&format!("output:{}", id), &p1)
                        .await?;
                    id += 1;
                    let nbytes = buffer.len();
                    if nbytes == 0 {
                        thread::sleep(core::time::Duration::from_millis(100));
                        continue;
                    }

                    *waiting_for_approval_clone.lock().unwrap() = false;
                    io::stderr().write_all(b"\r\x1b[K").unwrap();
                    io::stderr().flush().unwrap();
                    let mut last_cmd = last_cmd_clone.lock().unwrap();
                    let mn = min(last_cmd.len(), nbytes);
                    if mn > 0 && last_cmd[..mn] == buffer[..mn] {
                        last_cmd.drain(..mn);
                        drop(last_cmd);
                        io::stdout().write_all(&buffer[mn..nbytes]).unwrap();
                    } else {
                        last_cmd.clear();
                        drop(last_cmd);
                        io::stdout().write_all(&buffer[..nbytes]).unwrap();
                    }
                    io::stdout().flush().unwrap();
                }
                #[allow(unreachable_code)]
                Ok::<(), Box<dyn std::error::Error + Send + Sync + 'static>>(())
            })
            .unwrap();
    });
    // waiting_for_approval output
    let waiting_for_approval_clone = waiting_for_approval.clone();
    thread::spawn(move || {
        let mut i = 0;
        let c = ["\\", "|", "/", "-"];
        loop {
            let waiting_for_approval = *waiting_for_approval_clone.lock().unwrap();
            if waiting_for_approval {
                io::stderr()
                    .write_all(
                        format!("\r\x1b[01mWaiting for approval...{}\x1b[0m", c[i]).as_bytes(),
                    )
                    .unwrap();
                io::stderr().flush().unwrap();
            }
            thread::sleep(core::time::Duration::from_millis(250));
            i = (i + 1) % 4;
        }
    });

    // ctrlc
    let (tx, rx) = channel();
    ctrlc::set_handler(move || tx.send(()).unwrap()).unwrap();
    let cl_clone = cl.clone();
    let p1 = participants[1].clone();
    thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let mut id = 0;
                loop {
                    rx.recv()?;
                    cl_clone
                        .send_variable(&format!("ctrlc:{}", id), "".as_bytes(), &[p1.clone()])
                        .await?;
                    id += 1;
                }
                #[allow(unreachable_code)]
                Ok::<(), Box<dyn std::error::Error + Send + Sync + 'static>>(())
            })
            .unwrap();
    });

    // command
    let mut id = 0;
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let cmd = format!("{}\n", line?);
        cl.send_variable(
            &format!("command:{}", id),
            cmd.as_bytes(),
            &[participants[1].clone()],
        )
        .await?;
        let mut last_cmd_vec = last_cmd.lock().unwrap();
        last_cmd_vec.clear();
        last_cmd_vec.append(&mut cmd.as_bytes().to_vec());
        drop(last_cmd_vec);
        io::stderr()
            .write_all(b"\r\x1b[01mWaiting for approval...\x1b[0m")
            .unwrap();
        io::stderr().flush().unwrap();
        *waiting_for_approval.lock().unwrap() = true;
        id += 1;
    }
    cl.send_variable_with_remote_storage(
        &format!("command:{}", id),
        &[],
        &[participants[1].clone()],
    )
    .await?;
    io::stderr().write_all(b"exit\n").unwrap();
    io::stderr().flush().unwrap();
    Ok(())
}
