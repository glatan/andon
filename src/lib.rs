//! andon — Devin セッション群の状態を一覧し、人間への判断要求を中継する。
//!
//! セッションの実行そのものは Devin が行う。andon は「見る」(一覧・図解)と
//! 「選ぶ」(判断要求への回答の中継)だけを担当する。設計の正本は docs/design.md。

pub mod adapter;
pub mod answer;
pub mod card;
pub mod cli;
pub mod config;
pub mod error;
pub mod notify;
pub mod server;
pub mod store;
pub mod types;

pub use error::{Error, Result};
