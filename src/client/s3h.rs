#![allow(clippy::uninlined_format_args)]
mod common;
use colink::{decode_jwt_without_validation, CoLink, Participant};
use common::*;
use std::{
    io::{self, Write},
    process::exit,
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
    let cl_clone = cl.clone();
    let participants_clone = participants.clone();
    let task_id = tokio::spawn(async move {
        cl_clone
            .run_task("s3h", "".as_bytes(), &participants_clone, true)
            .await
    });

    // connecting
    let screen_host = connecting_msg(&cl, &target).await?;
    io::stderr()
        .write_all(
            format!(
                "\x1b[01mConnecting {}@{} ...\x1b[0m",
                &target[..7],
                screen_host
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

    exit(0);
}

async fn connecting_msg(cl: &CoLink, target: &str) -> Result<String, String> {
    io::stderr()
        .write_all(
            format!(
                "\x1b[01mResolving target host for user {} from registries...\x1b[0m",
                &target[..7]
            )
            .as_bytes(),
        )
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
                    "\r\x1b[01mResolving target host for user {} from registries...{}\x1b[0m",
                    &target[..7],
                    c[counter % 4]
                )
                .as_bytes(),
            )
            .unwrap();
        io::stderr().flush().unwrap();
        tokio::time::sleep(core::time::Duration::from_millis(250)).await;
        counter += 1;
        if counter > 480 {
            let msg = format!(
                "\n\x1b[01mTimeout: fail to find target {}\x1b[0m\n",
                &target[..7]
            );
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
        .write_all(format!("\n\x1b[01mFound target host: {}\x1b[0m\n", host).as_bytes())
        .unwrap();
    Ok(host.to_string())
}
