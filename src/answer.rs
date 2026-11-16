//! 判断カードへの回答の実行。server の HTTP ハンドラと cli の decide から共用。

use crate::adapter::devin_cloud::DevinCloud;
use crate::store::{Store, now};
use crate::types::DecisionState;
use crate::{Error, Result, card};

/// 選択肢 ID または自由入力の回答を Devin メッセージとして送り、
/// カードを answered にする。送信したメッセージ文を返す。
pub fn submit(
    store: &Store,
    devin: &DevinCloud,
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
    devin.send_message(&d.session_id, &message)?;
    store.answer_decision(id, &message, now())?;
    Ok(message)
}
