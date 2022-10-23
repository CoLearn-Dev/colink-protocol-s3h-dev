use colink::{decode_jwt_without_validation, CoLink, Participant};
use std::{
    cmp::min,
    io::{self, BufRead, Read, Write},
    net::TcpStream,
    process::exit,
    sync::{mpsc::channel, Arc, Mutex},
    thread,
};
use structopt::StructOpt;

#[derive(StructOpt)]
#[structopt(name = "CoLink-S3H", about = "CoLink-S3H")]
pub struct CommandLineArgs {
    /// Address of CoLink server
    #[structopt(short, long, env = "COLINK_CORE_ADDR")]
    pub addr: String,

    /// User JWT
    #[structopt(short, long, env = "COLINK_JWT")]
    pub jwt: String,

    /// Target
    pub target: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let CommandLineArgs { addr, jwt, target } = CommandLineArgs::from_args();

    // run task
    let mut cl = CoLink::new(&addr, &jwt);
    let user_id = decode_jwt_without_validation(&jwt).unwrap().user_id;
    let participants = vec![
        Participant {
            user_id,
            role: "client".to_string(),
        },
        Participant {
            user_id: target.clone(),
            role: "server".to_string(),
        },
    ];
    let task_id = cl
        .run_task("remote_shell", "".as_bytes(), &participants, true)
        .await?;

    // connecting
    let screen_host = connecting_msg(&cl, &target).await?;
    io::stderr()
        .write_all(format!("Connecting {}@{} ...", &target[..10], screen_host).as_bytes())
        .unwrap();
    io::stderr().flush().unwrap();
    cl.set_task_id(&task_id);
    let socket_port = cl.get_variable("socket_port", &participants[1]).await?;
    let socket_port = String::from_utf8_lossy(&socket_port).to_string();
    let screen_addr = format!("{}:{}", screen_host, socket_port);
    io::stderr().write_all(b"\n").unwrap();
    io::stderr().flush().unwrap();

    // output
    let waiting_for_approval = Arc::new(Mutex::new(false));
    let waiting_for_approval_clone = waiting_for_approval.clone();
    let last_cmd: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let last_cmd_clone = last_cmd.clone();
    thread::spawn(move || {
        let mut stream = TcpStream::connect(screen_addr).unwrap();
        let mut buffer = [0; 4096];
        loop {
            let nbytes = stream.read(&mut buffer).unwrap();
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
                    .write_all(format!("\rWaiting for approval...{}", c[i]).as_bytes())
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

    // command
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
        drop(last_cmd_vec);
        io::stderr()
            .write_all(b"\rWaiting for approval...")
            .unwrap();
        io::stderr().flush().unwrap();
        *waiting_for_approval.lock().unwrap() = true;
        id += 1;
    }
    cl.set_variable(&format!("command:{}", id), &[], &[participants[1].clone()])
        .await?;
    exit(0);
}

async fn connecting_msg(cl: &CoLink, target: &str) -> Result<String, String> {
    io::stderr()
        .write_all(format!("Detecting target {} from registries...", &target[..10]).as_bytes())
        .unwrap();
    io::stderr().flush().unwrap();
    let mut counter = 0;
    let c = ["\\", "|", "/", "-"];
    while cl
        .read_entry(&format!("_internal:known_users:{}:core_addr", &target))
        .await
        .is_err()
        || cl
            .read_entry(&format!("_internal:known_users:{}:guest_jwt", &target))
            .await
            .is_err()
    {
        io::stderr()
            .write_all(
                format!(
                    "\rDetecting target {} from registries...{}",
                    &target[..10],
                    c[counter % 4]
                )
                .as_bytes(),
            )
            .unwrap();
        io::stderr().flush().unwrap();
        tokio::time::sleep(core::time::Duration::from_millis(250)).await;
        counter += 1;
        if counter > 480 {
            let msg = format!("\nTimeout: fail to find target {}\n", &target[..10]);
            io::stderr().write_all(msg.as_bytes()).unwrap();
            return Err(msg);
        }
    }
    let core_addr = cl
        .read_entry(&format!("_internal:known_users:{}:core_addr", &target))
        .await
        .unwrap();
    let core_addr = String::from_utf8_lossy(&core_addr);
    let url = url::Url::parse(&core_addr).unwrap();
    let host = url.host_str().unwrap();
    io::stderr()
        .write_all(format!("\nFound target host: {}\n", host).as_bytes())
        .unwrap();
    Ok(host.to_string())
}
