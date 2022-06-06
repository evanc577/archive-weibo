use std::borrow::Cow;
use std::path::Path;

use anyhow::Result;
use futures::StreamExt;
use reqwest::header::COOKIE;
use reqwest::{Client, Url};
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{format_description, OffsetDateTime};
use tokio::io::AsyncWriteExt;
use tokio::{fs, process};

#[derive(Deserialize, Debug)]
struct Mymblog {
    data: WeiboData,
}

#[derive(Deserialize, Debug)]
struct WeiboData {
    list: Vec<WeiboPost>,
}

#[derive(Deserialize, Debug)]
struct WeiboPost {
    created_at: String,
    id: u64,
    user: WeiboUser,
    #[serde(rename = "text_raw")]
    text: String,
    #[serde(rename = "pic_ids")]
    pictures: Vec<String>,
    #[serde(rename = "url_struct")]
    urls: Option<Vec<WeiboUrl>>,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct WeiboUser {
    id: u64,
    screen_name: String,
    #[serde(rename = "avatar_hd")]
    avatar: String,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug)]
struct WeiboUrl {
    #[serde(rename = "url_title")]
    title: String,
    #[serde(rename = "long_url")]
    url: String,
}

pub async fn download(
    client: &Client,
    cookie: &str,
    uid: u64,
    dir: impl AsRef<Path>,
) -> Result<()> {
    let posts = get_posts(client, cookie, uid).await?;

    let results =
        futures::stream::iter(posts.iter().map(|p| download_post(client, p, dir.as_ref())))
            .buffer_unordered(20)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .filter(|r| r.is_err())
            .collect::<Vec<_>>();

    for r in results {
        eprintln!("{:?}", r);
    }

    Ok(())
}

async fn get_posts(client: &Client, cookie: &str, uid: u64) -> Result<Vec<WeiboPost>> {
    static URL: &str = "https://weibo.com/ajax/statuses/mymblog";
    let mut posts = Vec::new();

    for page in 1.. {
        println!("Page {}", page);
        let new_posts = client
            .get(URL)
            .query(&[("uid", &uid.to_string()), ("page", &page.to_string())])
            .header(COOKIE, format!("SUB={}", cookie))
            .send()
            .await?
            .json::<Mymblog>()
            .await?
            .data
            .list;

        if new_posts.is_empty() {
            break;
        }

        posts.extend(new_posts)
    }

    Ok(posts)
}

async fn download_post(client: &Client, post: &WeiboPost, dir: impl AsRef<Path>) -> Result<()> {
    // Parse and format date
    let parse_format = format_description::parse("[weekday repr:short] [month repr:short] [day] \
                                                 [hour repr:24]:[minute]:[second] \
                                                 [offset_hour sign:mandatory][offset_minute] [year]")?;
    let dt = OffsetDateTime::parse(&post.created_at, &parse_format)?;
    let date_format = format_description::parse("[year][month][day]")?;
    let date = dt.format(&date_format)?;

    // Generate output location
    let prefix = format!("{}-{}-{}", date, post.id, post.user.screen_name);
    let post_dir = dir.as_ref().join(&prefix);

    // Check if output location exists
    if post_dir.exists() {
        return Ok(());
    }

    // Create directory
    fs::create_dir_all(&post_dir).await?;

    // Download images
    for (i, img_id) in post.pictures.iter().enumerate() {
        let filename = format!("{}-img{:02}.jpg", &prefix, i + 1);
        let path = post_dir.join(&filename);
        download_image(client, img_id, path).await?;
    }

    // Download videos
    if let Some(u) = &post.urls {
        for (i, u) in u.iter().enumerate() {
            if let Some(u) = is_video(u) {
                let filename = format!("{}-vid{:02}", &prefix, i + 1);
                let path = post_dir.join(&filename);
                download_video(&u, path).await?;
            }
        }
    }

    // Write text
    {
        let filename = format!("{}-content.txt", &prefix);
        let path = post_dir.join(filename);
        let url = format!("https://m.weibo.cn/status/{}", post.id);
        let time = dt.format(&Rfc3339)?;

        let mut file = fs::File::create(path).await?;
        file.write_all(format!("url: {}\n", url).as_bytes()).await?;
        file.write_all(format!("user: {}\n", post.user.screen_name).as_bytes())
            .await?;
        file.write_all(format!("created_at: {}\n", time).as_bytes())
            .await?;
        if let Some(u) = &post.urls {
            for u in u {
                if !u.url.is_empty() {
                    file.write_all(format!("link: {}\n", u.url).as_bytes())
                        .await?;
                }
            }
        }
        file.write_all(format!("\n{}", post.text).as_bytes())
            .await?;
    }

    println!("Downloaded {}", post.id);
    Ok(())
}

fn is_video(u: &WeiboUrl) -> Option<Cow<str>> {
    if u.url.starts_with("https://video.weibo.com") {
        let fid = Url::parse(&u.url)
            .unwrap()
            .query_pairs()
            .find(|f| f.0 == "fid")
            .unwrap()
            .1
            .to_string();
        return Some(Cow::from(format!(
            "https://weibo.com/tv/show/{}?from=old_pc_videoshow",
            fid
        )));
    } else if u.url.starts_with("https://weibo.com/tv/show/") {
        return Some(Cow::from(&u.url));
    }

    None
}

async fn download_image(client: &Client, img_id: &str, path: impl AsRef<Path>) -> Result<()> {
    // Download
    let url = format!("https://wx2.sinaimg.cn/large/{img_id}.jpg");
    let data = client.get(url).send().await?.bytes().await?;

    // Write
    let mut file = fs::File::create(path.as_ref()).await?;
    file.write_all(&data).await.map_err(|_| {
        anyhow::anyhow!(format!(
            "Could not write to {}",
            path.as_ref().to_string_lossy()
        ))
    })?;

    Ok(())
}

async fn download_video(url: &str, path: impl AsRef<Path>) -> Result<()> {
    let status = process::Command::new("lux")
        .arg("--output-name")
        .arg(path.as_ref().file_name().unwrap())
        .arg("--output-path")
        .arg(path.as_ref().parent().unwrap())
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?
        .wait()
        .await?;

    if !status.success() {
        return Err(anyhow::anyhow!("Unable to download {}", url));
    }

    Ok(())
}
