//! ntfy.sh への push 通知。失敗は呼び出し側で握り潰してよい。

use crate::Result;

#[derive(Debug, Clone)]
pub struct Target {
    pub base: String,
    pub topic: String,
}

impl Target {
    pub fn send(&self, title: &str, body: &str, click_url: &str) -> Result<()> {
        let url = format!("{}/{}", self.base.trim_end_matches('/'), self.topic);
        let mut req = ureq::post(&url).header("Title", title);
        if !click_url.is_empty() {
            req = req.header("Click", click_url);
        }
        req.send(body)?;
        Ok(())
    }
}
