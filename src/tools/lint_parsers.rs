//! Lint 出力の言語別パーサ（TS `src/agents/review/lint-parsers.ts` の移植）
//!
//! bash 経由で得た lint 出力を、正規化された finding に変換する。

use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

/// 正規化された finding（`type` / `message` は従来の issues 形式と互換）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LintFinding {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    pub tool: &'static str,
    /// `typescript` / `python` / `unknown`
    pub language: &'static str,
}

// ESLint stylish: "  12:5  error  message  rule-name"
static ESLINT_ROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+([0-9]+):([0-9]+)\s+(error|warning)\s+(.*?)(?:\s{2,}(\S+))?\s*$").unwrap()
});
// tsc: "src/a.ts(12,5): error TS2322: message"
static TSC_ROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?)\(([0-9]+),([0-9]+)\): (error|warning) (TS[0-9]+): (.*)$").unwrap()
});
// flake8 / ruff concise: "a.py:12:5: E501 message"
static PY_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?):([0-9]+):([0-9]+): ([A-Z]+[0-9]+) (.*)$").unwrap());

/// 出力を行に分ける。Windows（cmd.exe）の CRLF に備えて行末の `\r` を落とす
/// （TS 版は `\n` だけで分けるため、CRLF だと tsc / flake8 の行に当たらなかった）。
fn lines(output: &str) -> impl Iterator<Item = &str> {
    output
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
}

fn severity(s: &str) -> &'static str {
    if s == "error" { "error" } else { "warning" }
}

fn num(s: &str) -> Option<u64> {
    s.parse().ok()
}

/// 先頭文字が W/C/N/D の flake8/ruff コードは warning、それ以外は error とみなす
fn python_severity(code: &str) -> &'static str {
    if code.starts_with(['W', 'C', 'N', 'D']) {
        "warning"
    } else {
        "error"
    }
}

/// ESLint stylish と tsc の出力を解釈する
pub fn parse_eslint_output(output: &str) -> Vec<LintFinding> {
    let mut findings = vec![];
    let mut file: Option<String> = None;
    for line in lines(output) {
        if let Some(c) = TSC_ROW.captures(line) {
            findings.push(LintFinding {
                kind: severity(&c[4]),
                message: c[6].to_string(),
                file: Some(c[1].to_string()),
                line: num(&c[2]),
                column: num(&c[3]),
                rule: Some(c[5].to_string()),
                tool: "tsc",
                language: "typescript",
            });
            continue;
        }
        if let Some(c) = ESLINT_ROW.captures(line) {
            findings.push(LintFinding {
                kind: severity(&c[3]),
                message: c[4].to_string(),
                file: file.clone(),
                line: num(&c[1]),
                column: num(&c[2]),
                rule: c.get(5).map(|m| m.as_str().to_string()),
                tool: "eslint",
                language: "typescript",
            });
        } else if !line.trim().is_empty()
            && !line.starts_with(char::is_whitespace)
            && !line.starts_with('✖')
        {
            // 字下げの無い行はファイル名の見出し
            file = Some(line.trim().to_string());
        }
    }
    findings
}

/// flake8 / ruff（concise）の出力を解釈する
pub fn parse_python_output(output: &str) -> Vec<LintFinding> {
    lines(output)
        .filter_map(|line| PY_ROW.captures(line))
        .map(|c| LintFinding {
            kind: python_severity(&c[4]),
            message: c[5].to_string(),
            file: Some(c[1].to_string()),
            line: num(&c[2]),
            column: num(&c[3]),
            rule: Some(c[4].to_string()),
            tool: "python-lint",
            language: "python",
        })
        .collect()
}

/// 従来の行単位フォールバック（`error` / `warning` を含む行）
fn parse_generic(output: &str) -> Vec<LintFinding> {
    lines(output)
        .filter(|l| l.contains("error") || l.contains("warning"))
        .map(|l| LintFinding {
            kind: if l.contains("error") {
                "error"
            } else {
                "warning"
            },
            message: l.to_string(),
            file: None,
            line: None,
            column: None,
            rule: None,
            tool: "generic",
            language: "unknown",
        })
        .collect()
}

/// 出力形式から言語を推定してパースする。どれにも当たらなければ汎用パーサ
pub fn parse_lint_findings(output: &str) -> Vec<LintFinding> {
    let parsers: [fn(&str) -> Vec<LintFinding>; 2] = [parse_eslint_output, parse_python_output];
    for parse in parsers {
        let found = parse(output);
        if !found.is_empty() {
            return found;
        }
    }
    parse_generic(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn as_json(f: &[LintFinding]) -> Vec<Value> {
        f.iter().map(|x| serde_json::to_value(x).unwrap()).collect()
    }

    #[test]
    fn parses_eslint_stylish_output() {
        let out = [
            "/repo/src/a.ts",
            "  12:5  error    Unexpected any  @typescript-eslint/no-explicit-any",
            "  20:1  warning  Missing semicolon  semi",
            "",
            "✖ 2 problems (1 error, 1 warning)",
        ]
        .join("\n");
        let f = as_json(&parse_lint_findings(&out));
        assert_eq!(f.len(), 2);
        assert_eq!(
            f[0],
            json!({
                "type": "error",
                "message": "Unexpected any",
                "file": "/repo/src/a.ts",
                "line": 12,
                "column": 5,
                "rule": "@typescript-eslint/no-explicit-any",
                "tool": "eslint",
                "language": "typescript",
            })
        );
        assert_eq!(f[1]["type"], "warning");
        assert_eq!(f[1]["rule"], "semi");
        assert_eq!(f[1]["message"], "Missing semicolon");
    }

    #[test]
    fn parses_tsc_output() {
        let f = parse_lint_findings("src/a.ts(3,7): error TS2322: Type 'x' is not assignable.");
        assert_eq!(
            as_json(&f)[0],
            json!({
                "type": "error",
                "message": "Type 'x' is not assignable.",
                "file": "src/a.ts",
                "line": 3,
                "column": 7,
                "rule": "TS2322",
                "tool": "tsc",
                "language": "typescript",
            })
        );
    }

    #[test]
    fn parses_flake8_ruff_output() {
        let f = as_json(&parse_lint_findings(
            "pkg/m.py:4:1: F401 os imported but unused\npkg/m.py:9:80: E501 line too long",
        ));
        assert_eq!(f.len(), 2);
        assert_eq!(f[0]["type"], "error");
        assert_eq!(f[0]["rule"], "F401");
        assert_eq!(f[0]["language"], "python");
        assert_eq!(f[0]["tool"], "python-lint");
        assert_eq!(f[0]["line"], 4);
        assert_eq!(f[0]["file"], "pkg/m.py");
        assert_eq!(f[1]["column"], 80);
    }

    #[test]
    fn python_severity_by_code_prefix() {
        let f = parse_python_output(
            "a.py:1:1: W291 trailing\na.py:2:1: C901 complex\na.py:3:1: N801 name\na.py:4:1: D100 doc\na.py:5:1: E1 x",
        );
        let kinds: Vec<_> = f.iter().map(|x| x.kind).collect();
        assert_eq!(kinds, ["warning", "warning", "warning", "warning", "error"]);
    }

    #[test]
    fn falls_back_to_generic_parsing() {
        let f = parse_lint_findings("something error happened\nok");
        assert_eq!(
            as_json(&f),
            vec![json!({
                "type": "error",
                "message": "something error happened",
                "tool": "generic",
                "language": "unknown",
            })]
        );
    }

    #[test]
    fn eslint_parser_alone_ignores_python_rows() {
        assert!(parse_eslint_output("pkg/m.py:4:1: F401 unused").is_empty());
        assert_eq!(parse_python_output("pkg/m.py:4:1: F401 unused").len(), 1);
    }

    #[test]
    fn crlf_output_is_parsed() {
        // Windows（cmd.exe）の出力は CRLF。行末の \r は落としてから解釈する
        let f = parse_lint_findings(
            "src/a.ts(3,7): error TS2322: bad\r\nsrc/b.ts(1,1): warning TS1: w\r\n",
        );
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].message, "bad");
        assert_eq!(f[1].kind, "warning");
    }
}
