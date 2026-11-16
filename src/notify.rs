//! ntfy.sh への push 通知。失敗は呼び出し側で握り潰してよい。

use crate::Result;

#[derive(Debug, Clone)]
pub struct Target {
    pub base: String,
    pub topic: String,
    /// 設定時はセッション URL より優先して通知のタップ先になる。
    /// スマホから見える andon の URL(例: tailscale 経由)を入れる。
    pub public_url: Option<String>,
}

impl Target {
    pub fn send(&self, title: &str, body: &str, click_url: &str) -> Result<()> {
        let url = format!("{}/{}", self.base.trim_end_matches('/'), self.topic);
        let mut req = ureq::post(&url).header("Title", title);
        let click = self.public_url.as_deref().unwrap_or(click_url);
        if !click.is_empty() {
            req = req.header("Click", click);
        }
        req.send(body)?;
        Ok(())
    }
}
