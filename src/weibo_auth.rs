use anyhow::Result;
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;

pub async fn weibo_cookie(client: &Client) -> Result<String> {
    let tid = get_tid(client).await?;
    let cookie = get_cookie(client, &tid).await?;
    Ok(cookie)
}

async fn get_tid(client: &Client) -> Result<String> {
    static URL: &str = "https://passport.weibo.com/visitor/genvisitor";
    let re = Regex::new(r"gen_callback\((.*)\);").unwrap();

    #[derive(Deserialize)]
    struct ResponseJson {
        data: Data,
    }

    #[derive(Deserialize)]
    struct Data {
        tid: String,
    }

    // POST to Weibo
    let text = client
        .post(URL)
        .form(&[("cb", "gen_callback")])
        .send()
        .await?
        .text()
        .await?;

    // Extract and parse JSON
    let json: ResponseJson = serde_json::from_str(
        re.captures(&text)
            .ok_or_else(|| anyhow::anyhow!("JSON not found"))?
            .get(1)
            .unwrap()
            .as_str(),
    )?;

    Ok(json.data.tid)
}

async fn get_cookie(client: &Client, tid: &str) -> Result<String> {
    static URL: &str = "https://passport.weibo.com/visitor/visitor";

    // GET to Weibo
    let resp = client
        .get(URL)
        .query(&[("a", "incarnate"), ("t", tid)])
        .send()
        .await?;

    // Extract SUB cookie
    let cookie = resp
        .cookies()
        .find(|c| c.name() == "SUB")
        .ok_or_else(|| anyhow::anyhow!("SUB cookie not found"))?
        .value()
        .to_owned();

    Ok(cookie)
}
