use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use futures::StreamExt;
use reqwest::Client;

use crate::weibo_post::WeiboPost;

mod weibo_auth;
mod weibo_post;

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

    println!("Getting posts");
    let posts = WeiboPost::get_posts(&client, args.user).await?;

    println!("Downloading posts");
    futures::stream::iter(posts.iter().map(|p| p.download(&client, &args.dir)))
        .buffer_unordered(20)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter(|r| r.is_err())
        .for_each(|e| eprintln!("{:?}", e));

    Ok(())
}
