use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use reqwest::Client;

mod weibo_auth;
mod weibo_download;

#[derive(Parser, Debug)]
struct Args {
    #[clap(short, long)]
    user: u64,

    #[clap(short, long, default_value = ".")]
    dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let client = Client::new();
    
    println!("Authenticating");
    let cookie = weibo_auth::weibo_cookie(&client).await?;

    println!("Getting posts");
    weibo_download::download(&client, &cookie, args.user, args.dir).await?;

    Ok(())
}
