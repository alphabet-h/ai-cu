//! ai-cu: AI が JSON の操作列で Windows のデスクトップを操作するための CLI のライブラリ部分。
//!
//! 「画面を見て次を決める」ループは呼び出し側の AI が担い、このクレートは「手」(操作の実行) と
//! 「目」(画面の取得) だけを担う。実入力の経路には、設定の allow / deny、事前検証、フォアグラウンドガード、
//! 緊急停止、開発シェルでの実行時ガードを通す (README.md の「安全機能」)。

#![cfg(windows)]

pub mod backend;
pub mod build_info;
pub mod cli;
pub mod config;
pub mod device;
pub mod elements;
pub mod error;
pub mod estop;
pub mod focus_helper;
pub mod geometry;
pub mod hint;
pub mod image;
pub mod jsonrpc;
pub mod keys;
pub mod mcp;
pub mod plan;
pub mod policy;
pub mod run;
pub mod runner;
pub mod script;
#[doc(hidden)]
pub mod testing;
pub mod tui;
pub mod validate;
pub mod win32;
