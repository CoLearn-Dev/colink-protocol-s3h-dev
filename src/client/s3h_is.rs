#![allow(clippy::uninlined_format_args)]
mod common;
use colink::{
    decode_jwt_without_validation,
    extensions::policy_module::{Action, Rule, TaskFilter},
    CoLink, Participant,
};
use common::*;
use rand::Rng;
use std::{
    io::{self, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::exit,
    process::{Child, Command},
};
use structopt::StructOpt;

pub struct ChildProcess {
    pub process: Child,
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.process.kill().unwrap();
    }
}

#[derive(StructOpt)]
#[structopt(name = "S3H", about = "S3H")]
pub struct CommandLineArgs {
    /// Address of CoLink server
    #[structopt(short, long, env = "S3H_ADDR")]
    pub addr: String,

    #[structopt(short, long, env = "S3H_PORT", default_value = "2222")]
    pub port: u16,

    /// Guest JWT
    #[structopt(short, long, env = "S3H_GUEST_JWT")]
    pub guest_jwt: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let CommandLineArgs {
        addr,
        port,
        guest_jwt,
    } = CommandLineArgs::from_args();

    // instant server
    let colink_home = Path::new(&std::env::current_dir()?).join(".s3h_colink_home");
    let mut is_port = rand::thread_rng().gen_range(10000..20000);
    while std::net::TcpStream::connect(format!("127.0.0.1:{}", is_port)).is_ok() {
        is_port = rand::thread_rng().gen_range(10000..20000);
    }
    let instant_server_id = uuid::Uuid::new_v4().to_string();
    let working_dir = Path::new(&colink_home)
        .join("instant_servers")
        .join(instant_server_id.clone());
    std::fs::create_dir_all(&working_dir).unwrap();
    let program = Path::new(&colink_home).join("colink-server");
    if std::fs::metadata(program.clone()).is_err() {
        Command::new("bash")
                .arg("-c")
                .arg("bash -c \"$(curl -fsSL https://raw.githubusercontent.com/CoLearn-Dev/colinkctl/main/install_colink.sh)\"")
                .env("COLINK_INSTALL_SERVER_ONLY", "true")
                .env("COLINK_INSTALL_SILENT", "true")
                .env("COLINK_SERVER_VERSION", "v0.3.3")
                .env("COLINK_HOME", &colink_home)
                .status()
                .unwrap();
    }
    let mut file = std::fs::File::create(Path::new(&colink_home).join("user_init_config.toml"))?;
    file.write_all(b"")?;
    let args = vec![
        "--address".to_string(),
        "0.0.0.0".to_string(),
        "--port".to_string(),
        is_port.to_string(),
        "--mq-prefix".to_string(),
        format!("colink-instant-server-{}", is_port),
        "--core-uri".to_string(),
        format!("http://127.0.0.1:{}", is_port),
        "--inter-core-reverse-mode".to_string(),
    ];
    let child = Command::new(program)
        .args(&args)
        .env("COLINK_HOME", &colink_home)
        .current_dir(working_dir.clone())
        .process_group(0)
        .spawn()
        .unwrap();
    let is = ChildProcess { process: child };
    loop {
        if std::fs::metadata(working_dir.join("host_token.txt")).is_ok()
            && std::net::TcpStream::connect(format!("127.0.0.1:{}", is_port)).is_ok()
        {
            break;
        }
        std::thread::sleep(core::time::Duration::from_millis(10));
    }
    let host_token: String =
        String::from_utf8_lossy(&std::fs::read(working_dir.join("host_token.txt")).unwrap())
            .parse()
            .unwrap();
    // end instant server

    let mut cl = CoLink::new(&format!("http://127.0.0.1:{}", is_port), &host_token)
        .switch_to_generated_user()
        .await?;
    let user_id = cl.get_user_id()?;
    cl.start_protocol_operator("policy_module", &user_id, false)
        .await?;
    cl.policy_module_add_rule(&Rule {
        task_filter: Some(TaskFilter::default()),
        action: Some(Action {
            r#type: "approve".to_string(),
            ..Default::default()
        }),
        priority: 1,
        ..Default::default()
    })
    .await?;
    cl.start_protocol_operator("remote_storage", &user_id, false)
        .await?;

    // run task
    let target_user_id = decode_jwt_without_validation(&guest_jwt).unwrap().user_id;
    cl.import_core_addr(&target_user_id, &format!("http://{}:{}", addr, port))
        .await?;
    cl.import_guest_jwt(&guest_jwt).await?;
    let participants = vec![
        Participant {
            user_id,
            role: "client".to_string(),
        },
        Participant {
            user_id: target_user_id.clone(),
            role: "server".to_string(),
        },
    ];
    let cl_clone = cl.clone();
    let participants_clone = participants.clone();
    let task_id = tokio::spawn(async move {
        cl_clone
            .run_task("s3h", Default::default(), &participants_clone, true)
            .await
    });

    // connecting
    io::stderr()
        .write_all(
            format!(
                "\x1b[01mConnecting {}@{} ...\x1b[0m",
                &target_user_id[..7],
                addr
            )
            .as_bytes(),
        )
        .unwrap();
    io::stderr().flush().unwrap();
    let task_id = task_id.await??;
    cl.set_task_id(&task_id);
    cl.recv_variable("start_session", &participants[1]).await?;
    io::stderr().write_all(b"\n").unwrap();
    io::stderr().flush().unwrap();

    s3h_session(&cl, &participants).await?;

    drop(is);
    exit(0);
}
