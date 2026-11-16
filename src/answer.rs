//! 判断カードへの回答の実行。server の HTTP ハンドラと cli の decide から共用。

use crate::adapter::devin_acp::{DevinAcp, FINGERPRINT_PERM};
use crate::adapter::devin_cloud::DevinCloud;
use crate::adapter::devin_local::{DevinLocal, LOCAL_PREFIX};
use crate::store::{Store, now};
use crate::types::DecisionState;
use crate::{Error, Result, card};

/// 選択肢 ID または自由入力の回答を送信先へ中継し、カードを answered にする。
/// 送信先は fingerprint/session_id で振り分ける:
/// `acp-perm:` = ACP 権限要求、`local:` = ACP prompt(優先)または devin -r、
/// それ以外 = Cloud messages API。送信した内容を返す。
#[allow(clippy::too_many_arguments)]
pub fn submit(
    store: &Store,
    devin: Option<&DevinCloud>,
    local: Option<&DevinLocal>,
    acp: Option<&DevinAcp>,
    id: i64,
    option_id: Option<&str>,
    text: Option<&str>,
) -> Result<String> {
    let d = store.decision(id)?.ok_or(Error::NotFound(id))?;
    if d.state != DecisionState::Pending {
        return Err(Error::AlreadyAnswered(id));
    }
    let option = option_id.and_then(|oid| d.options.iter().find(|o| o.id == oid));
    if let (Some(oid), None) = (option_id, &option) {
        return Err(Error::BadRequest(format!(
            "option_id がカードの選択肢と一致しません: {oid}"
        )));
    }
    let text = text.filter(|t| !t.is_empty());
    if d.fingerprint.starts_with(FINGERPRINT_PERM) {
        let acp =
            acp.ok_or_else(|| Error::Local("ACP 未接続のため権限要求に回答できません".to_owned()))?;
        acp.respond_permission(&d.fingerprint, option.map(|o| o.id.as_str()))?;
        // 自由入力は ACP 側では cancelled だが、入力内容は監査のため記録する。
        let sent = option
            .map(|o| o.label.clone())
            .or_else(|| text.map(str::to_owned))
            .unwrap_or_else(|| "キャンセル".to_owned());
        store.answer_decision(id, &sent, now())?;
        return Ok(sent);
    }
    let message = card::answer_message(option, text);
    if let Some(raw) = d.session_id.strip_prefix(LOCAL_PREFIX) {
        let local = local.ok_or_else(|| {
            Error::Local("serve --source local/both で起動していません".to_owned())
        })?;
        let cwd = local.session_cwd(raw)?;
        // cwd が不明で未ロードのセッションへ ACP prompt を投げると
        // 応答が捨てられ黙って失敗するため、その場合は devin -r を使う。
        if let Some(acp) = acp.filter(|a| cwd.is_some() || a.is_loaded(raw)) {
            match acp.prompt(raw, cwd.as_deref(), &message) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("[andon] ACP 注入に失敗、devin -r にフォールバック: {e}");
                    local.resume(&d.session_id, &message)?;
                }
            }
        } else {
            local.resume(&d.session_id, &message)?;
        }
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
