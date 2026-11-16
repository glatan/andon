//! 判断カードへの回答の実行。server の HTTP ハンドラと cli の decide から共用。

use crate::adapter::devin_cloud::DevinCloud;
use crate::adapter::devin_local::{DevinLocal, LOCAL_PREFIX};
use crate::store::{Store, now};
use crate::types::DecisionState;
use crate::{Error, Result, card};

/// 選択肢 ID または自由入力の回答を Devin へ送り、カードを answered にする。
/// `local:` 接頭辞のセッションは `devin -r -p`、それ以外は Cloud API で送信。
/// 送信したメッセージ文を返す。
pub fn submit(
    store: &Store,
    devin: Option<&DevinCloud>,
    local: Option<&DevinLocal>,
    id: i64,
    option_id: Option<&str>,
    text: Option<&str>,
) -> Result<String> {
    let d = store.decision(id)?.ok_or(Error::NotFound(id))?;
    if d.state != DecisionState::Pending {
        return Err(Error::AlreadyAnswered(id));
    }
    let option = option_id.and_then(|oid| d.options.iter().find(|o| o.id == oid));
    let message = card::answer_message(option, text.filter(|t| !t.is_empty()));
    if let Some(raw) = d.session_id.strip_prefix(LOCAL_PREFIX) {
        local
            .ok_or_else(|| Error::Local("serve --source local/both で起動していません".to_owned()))?
            .resume(&d.session_id, &message)?;
        // 回答に成功したセッションは以後のターン終了でもカード化対象にする。
        store.meta_set(&format!("local_owned:{raw}"), "1")?;
    } else {
        devin
            .ok_or(Error::Config("DEVIN_API_KEY"))?
            .send_message(&d.session_id, &message)?;
    }
    store.answer_decision(id, &message, now())?;
    Ok(message)
}
