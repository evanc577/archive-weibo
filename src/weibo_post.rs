use std::borrow::Cow;
use std::path::Path;

use anyhow::Result;
use once_cell::sync::Lazy;
use reqwest::header::COOKIE;
use reqwest::{Client, Url};
use serde::{Deserialize, Deserializer};
use time::format_description::well_known::Rfc3339;
use time::format_description::FormatItem;
use time::{format_description, OffsetDateTime};
use tokio::io::AsyncWriteExt;
use tokio::{fs, process};

use crate::weibo_auth::weibo_cookie;

#[derive(Deserialize, Debug)]
struct Mymblog {
    data: WeiboData,
}

#[derive(Deserialize, Debug)]
struct WeiboData {
    list: Vec<WeiboPost>,
}

#[derive(Deserialize, Debug)]
pub struct WeiboPost {
    #[serde(deserialize_with = "deserialize_datetime")]
    created_at: OffsetDateTime,
    id: u64,
    user: WeiboUser,
    #[serde(rename = "text_raw")]
    text: String,
    #[serde(rename = "pic_ids")]
    pictures: Vec<String>,
    #[serde(rename = "url_struct")]
    urls: Option<Vec<WeiboUrl>>,
}

fn deserialize_datetime<'de, D>(deserializer: D) -> Result<OffsetDateTime, D::Error>
where
    D: Deserializer<'de>,
{
    static FMT: &str = concat!(
        "[weekday repr:short] [month repr:short] [day] ",
        "[hour repr:24]:[minute]:[second] ",
        "[offset_hour sign:mandatory][offset_minute] [year]",
    );
    static PARSE_FORMAT: Lazy<Vec<FormatItem>> =
        Lazy::new(|| format_description::parse(FMT).unwrap());
    let s = String::deserialize(deserializer)?;
    OffsetDateTime::parse(&s, &PARSE_FORMAT).map_err(serde::de::Error::custom)
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

impl WeiboPost {
    pub async fn get_posts(client: &Client, uid: u64) -> Result<Vec<Self>> {
        static URL: &str = "https://weibo.com/ajax/statuses/mymblog";
        let mut posts = Vec::new();
        let cookie = weibo_cookie(client).await?;

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

    pub async fn download(&self, client: &Client, dir: impl AsRef<Path>) -> Result<()> {
        // Generate output location
        let prefix = self.gen_prefix()?;
        let post_dir = dir.as_ref().join(&prefix);

        // Check if output location exists
        if post_dir.exists() {
            return Ok(());
        }

        // Create directory
        fs::create_dir_all(&post_dir).await?;

        // Download images
        for (i, img_id) in self.pictures.iter().enumerate() {
            let filename = format!("{}-img{:02}.jpg", &prefix, i + 1);
            let path = post_dir.join(&filename);
            download_image(client, img_id, path).await?;
        }

        // Download videos
        if let Some(u) = &self.urls {
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
            self.write_text(path).await?;
        }

        println!("Downloaded {}", self.id);
        Ok(())
    }

    fn gen_prefix(&self) -> Result<String> {
        static FORMAT: Lazy<Vec<FormatItem>> =
            Lazy::new(|| format_description::parse("[year][month][day]").unwrap());
        let date = self.created_at.format(&FORMAT)?;
        let prefix = format!("{}-{}-{}", date, self.id, self.user.screen_name);
        Ok(prefix)
    }

    async fn write_text(&self, path: impl AsRef<Path>) -> Result<()> {
        let url = format!("https://m.weibo.cn/status/{}", self.id);
        let time = self.created_at.format(&Rfc3339)?;

        let mut file = fs::File::create(path).await?;
        file.write_all(format!("url: {}\n", url).as_bytes()).await?;
        file.write_all(format!("user: {}\n", self.user.screen_name).as_bytes())
            .await?;
        file.write_all(format!("created_at: {}\n", time).as_bytes())
            .await?;
        if let Some(u) = &self.urls {
            for u in u {
                if !u.url.is_empty() {
                    file.write_all(format!("link: {}\n", u.url).as_bytes())
                        .await?;
                }
            }
        }
        file.write_all(format!("\n{}", self.text).as_bytes())
            .await?;

        Ok(())
    }
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
