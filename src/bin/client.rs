use colink::{decode_jwt_without_validation, CoLink, Participant};
use std::{
    cmp::min,
    env,
    io::{self, BufRead, Read, Write},
    net::TcpStream,
    process::exit,
    sync::{mpsc::channel, Arc, Mutex},
    thread,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let addr = &args[0];
    let jwt = &args[1];
    let target = &args[2];

    let mut cl = CoLink::new(addr, jwt);
    let user_id = decode_jwt_without_validation(jwt).unwrap().user_id;
    let target = match decode_jwt_without_validation(target) {
        Ok(content) => content.user_id,
        Err(_) => target.to_string(),
    };

    let participants = vec![
        Participant {
            user_id,
            role: "client".to_string(),
        },
        Participant {
            user_id: target,
            role: "server".to_string(),
        },
    ];
    let task_id = cl
        .run_task("remote_shell", "".as_bytes(), &participants, true)
        .await?;

    cl.set_task_id(&task_id);
    let screen_addr = cl.get_variable("screen", &participants[1]).await?;
    let screen_addr = String::from_utf8_lossy(&screen_addr).to_string();

    let last_cmd: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let last_cmd_clone = last_cmd.clone();
    thread::spawn(move || {
        let mut stream = TcpStream::connect(screen_addr).unwrap();
        let mut buffer = [0; 4096];
        loop {
            let nbytes = stream.read(&mut buffer).unwrap();
            if nbytes == 0 {
                thread::sleep(core::time::Duration::from_millis(100));
            }
            io::stderr()
                .write_all(b"\r                       \r")
                .unwrap();
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
    });

    let (tx, rx) = channel();
    ctrlc::set_handler(move || tx.send(()).unwrap()).unwrap();
    let cl_clone = cl.clone();
    let p1 = participants[1].clone();
    tokio::spawn(async move {
        let mut id = 0;
        loop {
            rx.recv()?;
            cl_clone
                .set_variable(&format!("ctrlc:{}", id), "".as_bytes(), &[p1.clone()])
                .await?;
            id += 1;
        }
        #[allow(unreachable_code)]
        Ok::<(), Box<dyn std::error::Error + Send + Sync + 'static>>(())
    });

    let mut id = 0;
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let cmd = format!("{}\n", line?);
        cl.set_variable(
            &format!("command:{}", id),
            cmd.as_bytes(),
            &[participants[1].clone()],
        )
        .await?;
        let mut last_cmd_vec = last_cmd.lock().unwrap();
        last_cmd_vec.clear();
        last_cmd_vec.append(&mut cmd.as_bytes().to_vec());
        io::stderr()
            .write_all(b"\rWaiting for approval...")
            .unwrap();
        drop(last_cmd_vec);
        io::stderr().flush().unwrap();
        id += 1;
    }
    cl.set_variable(&format!("command:{}", id), &[], &[participants[1].clone()])
        .await?;
    exit(0);
}
