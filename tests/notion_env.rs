//! `NOTION_TOKEN` は呼び出し時に環境変数から読む（TS #45 と同じ）。
//! 環境変数を書き換えるので、他のテストと並列にならないよう専用のテストバイナリに置く。

use subagent::tools::notion::token_from_env;

#[test]
fn token_is_read_from_env_at_call_time() {
    // SAFETY: このテストバイナリには他のテストもスレッドも無い
    unsafe { std::env::remove_var("NOTION_TOKEN") };
    assert_eq!(token_from_env(), None);
    unsafe { std::env::set_var("NOTION_TOKEN", "") };
    assert_eq!(token_from_env(), None, "空文字列は未設定扱い");
    unsafe { std::env::set_var("NOTION_TOKEN", "secret_x") };
    assert_eq!(token_from_env().as_deref(), Some("secret_x"));
    unsafe { std::env::remove_var("NOTION_TOKEN") };
}
