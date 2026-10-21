//! Extract paper identifiers from pasted chat / share-page text (#652).
//!
//! Deterministic scanner (no LLM, no network): matches only unambiguous
//! identifier forms so precision stays high in noisy chat text —
//!
//! - arXiv: `arxiv.org/(abs|pdf|html|src)/<id>` URLs and `arXiv:<id>` mentions
//! - DOI: `doi.org/<doi>` URLs (validated via [`clean_doi`])
//! - PMID: `pubmed.ncbi.nlm.nih.gov/<pmid>` URLs and `PMID[: ]<pmid>` mentions
//!
//! Deliberately excluded: bare `2403.15388`-style arXiv numbers and bare
//! `10.x/yyyy` DOIs without a `doi.org` prefix — both false-positive heavily
//! in chat text. Duplicates collapse in first-seen order.

use crate::error::AppError;
use crate::features::scholar_api::identifiers::{clean_doi, strip_arxiv_version};

/// Identifiers extracted from one blob of text, first-seen order.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ExtractedIdentifiers {
    /// Version-stripped arXiv ids (e.g. `2403.15388`).
    pub arxiv: Vec<String>,
    /// DOIs without the `https://doi.org/` prefix.
    pub dois: Vec<String>,
    /// Bare PubMed ids.
    pub pmids: Vec<String>,
}

impl ExtractedIdentifiers {
    /// Total identifier count.
    pub fn len(&self) -> usize {
        self.arxiv.len() + self.dois.len() + self.pmids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Characters that reliably end an identifier inside chat text. `&`/`?`/`=`
/// only occur in URL query strings, never in a DOI suffix or arXiv id, so they
/// end identifiers too — HTML-decoded share pages carry `doi.org/<doi>&amp;…`
/// tracking tails that would otherwise corrupt the extracted DOI.
fn ident_end(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '&' | '?'
                | '='
                | '"'
                | '\''
                | '<'
                | '>'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '|'
                | '`'
                | ','
                | ';'
                | '，'
                | '。'
                | '、'
                | '”'
                | '“'
                | '‘'
                | '’'
                | '（'
                | '）'
                | '：'
                | '；'
                | '！'
                | '？'
        )
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if !value.is_empty() && !list.contains(&value) {
        list.push(value);
    }
}

fn read_arxiv_tail(rest: &str) -> String {
    // <digits>.<digits> with optional vN
    let mut out = String::new();
    let mut seen_dot = false;
    let mut digits_before_dot = 0usize;
    for c in rest.chars() {
        if c.is_ascii_digit() && !seen_dot {
            digits_before_dot += 1;
            out.push(c);
        } else if c == '.' && !seen_dot && digits_before_dot == 4 {
            seen_dot = true;
            out.push(c);
        } else if c.is_ascii_digit() && seen_dot {
            out.push(c);
        } else if c == 'v' && seen_dot {
            // version suffix: keep reading digits
            out.push(c);
        } else {
            break;
        }
    }
    let valid = seen_dot
        && digits_before_dot == 4
        && out.ends_with(|c: char| c.is_ascii_digit() || c == 'v');
    if valid {
        out
    } else {
        String::new()
    }
}

/// Scan chat/share text for paper identifiers.
pub fn extract_identifiers(text: &str) -> ExtractedIdentifiers {
    let mut out = ExtractedIdentifiers::default();
    // Chinese input methods frequently emit a full-width colon after
    // `arXiv`/`PMID` mentions; normalize so both mention forms parse.
    let hay = text
        .to_ascii_lowercase()
        .replace("arxiv：", "arxiv:")
        .replace("pmid：", "pmid:");

    // arXiv URLs: arxiv.org/{abs|pdf|html|src}/<id>
    let mut rest = hay.as_str();
    while let Some(idx) = rest.find("arxiv.org/") {
        let after = &rest[idx + "arxiv.org/".len()..];
        let after = after
            .strip_prefix("abs/")
            .or_else(|| after.strip_prefix("pdf/"))
            .or_else(|| after.strip_prefix("html/"))
            .or_else(|| after.strip_prefix("src/"));
        if let Some(after) = after {
            let id = read_arxiv_tail(after);
            if !id.is_empty() {
                push_unique(&mut out.arxiv, strip_arxiv_version(&id));
            }
        }
        rest = &rest[idx + "arxiv.org/".len()..];
    }

    // arXiv: mentions
    let mut rest = hay.as_str();
    while let Some(idx) = rest.find("arxiv:") {
        let after = &rest[idx + "arxiv:".len()..];
        let after = after.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        let id = read_arxiv_tail(after);
        if !id.is_empty() {
            push_unique(&mut out.arxiv, strip_arxiv_version(&id));
        }
        rest = &rest[idx + "arxiv:".len()..];
    }

    // DOI URLs: doi.org/<doi>
    let mut rest = hay.as_str();
    while let Some(idx) = rest.find("doi.org/") {
        let after = &rest[idx + "doi.org/".len()..];
        let tail: String = after
            .chars()
            .take_while(|c| !ident_end(*c))
            .take(120)
            .collect();
        if let Some(doi) = clean_doi(&tail) {
            push_unique(&mut out.dois, doi);
        }
        rest = &rest[idx + "doi.org/".len()..];
    }

    // PubMed URLs
    let mut rest = hay.as_str();
    while let Some(idx) = rest.find("pubmed.ncbi.nlm.nih.gov/") {
        let after = &rest[idx + "pubmed.ncbi.nlm.nih.gov/".len()..];
        let digits: String = after
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .take(9)
            .collect();
        if digits.len() >= 5 {
            push_unique(&mut out.pmids, digits);
        }
        rest = &rest[idx + "pubmed.ncbi.nlm.nih.gov/".len()..];
    }

    // PMID mentions
    let mut rest = hay.as_str();
    while let Some(idx) = rest.find("pmid") {
        let after = &rest[idx + "pmid".len()..];
        let after = after.trim_start_matches([':', ' ', '\t']);
        let digits: String = after
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .take(9)
            .collect();
        if digits.len() >= 5 {
            push_unique(&mut out.pmids, digits);
        }
        rest = &rest[idx + "pmid".len()..];
    }

    out
}

/// True when `text` is exactly a public AI-chat share URL whose page renders
/// server-side, so a plain HTTP GET yields the conversation text. ChatGPT
/// first (#652); Claude/Gemini share pages need their own adapters and are
/// deliberately not claimed here.
pub fn is_share_url(text: &str) -> bool {
    let t = text.trim();
    if t.contains(char::is_whitespace) {
        return false;
    }
    let lowered = t.to_ascii_lowercase();
    let known = lowered.starts_with("https://chatgpt.com/share/")
        || lowered.starts_with("https://chat.openai.com/share/");
    known && lowered.rsplit('/').next().is_some_and(|id| !id.is_empty())
}

/// chatgpt.com serves Cloudflare challenges to non-browser agents; a desktop
/// Chrome UA passes for share pages (verified against a real share link).
const SHARE_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// Fetch a share page over HTTPS and return its server-rendered HTML.
/// Cloudflare fingerprints non-browser TLS stacks even behind a real UA, so
/// send the full browser header set — the combo matters as much as the UA.
async fn fetch_share_page(url: &str) -> Result<String, AppError> {
    let client = reqwest::Client::builder()
        .user_agent(SHARE_UA)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::Message(format!("share http client: {e}")))?;
    let resp = client
        .get(url)
        .header("accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8")
        .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
        .header("sec-ch-ua", "\"Chromium\";v=\"130\", \"Google Chrome\";v=\"130\"")
        .header("sec-ch-ua-mobile", "?0")
        .header("sec-ch-ua-platform", "\"Windows\"")
        .header("sec-fetch-dest", "document")
        .header("sec-fetch-mode", "navigate")
        .header("sec-fetch-site", "none")
        .header("sec-fetch-user", "?1")
        .header("upgrade-insecure-requests", "1")
        .send()
        .await
        .map_err(|e| AppError::Message(format!("share fetch failed: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::Message(format!(
            "share fetch returned HTTP {status}"
        )));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| AppError::Message(format!("share read failed: {e}")))?;
    if body.contains("Just a moment...") {
        return Err(AppError::Message(
            "share page served a bot challenge; paste the conversation text instead".into(),
        ));
    }
    Ok(body)
}

/// Fetch a share URL and extract identifiers from its server-rendered text.
/// The conversation body stays in memory only — identifiers are returned,
/// the page text is dropped. No HTML entity decoding is needed: `&` already
/// ends identifiers, so `&amp;`-suffixed tracking tails are cut by the
/// scanner itself.
pub async fn extract_from_share_url(url: &str) -> Result<ExtractedIdentifiers, AppError> {
    let html = fetch_share_page(url).await?;
    Ok(extract_identifiers(&html))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Realistic AI-chat snippet: full links, mentions, duplicates, noise,
    /// Chinese punctuation, and a non-paper URL that must not match.
    const CHAT_SAMPLE: &str = "我最近看了几篇：
1. https://arxiv.org/abs/2403.15388 讲 token 合并的
2. arXiv:1706.03762 就是那篇 Attention
3. Nature 那篇：https://doi.org/10.1038/nature12373，
4. PubMed 上的 https://pubmed.ncbi.nlm.nih.gov/38729424/ 也可以
5. 又贴了一遍 2403 的（重复）：https://arxiv.org/pdf/2403.15388v2
6. 参考 https://www.nature.com/articles/nature12373（不是 doi.org，不应入列）
7. PMID: 31245678
欢迎补充！";

    #[test]
    fn extracts_all_forms_and_deduplicates() {
        let out = extract_identifiers(CHAT_SAMPLE);
        assert_eq!(out.arxiv, vec!["2403.15388", "1706.03762"]);
        assert_eq!(out.dois, vec!["10.1038/nature12373"]);
        // dedup vs the PMID: mention below
        assert_eq!(out.pmids, vec!["38729424", "31245678"]);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn chinese_punctuation_boundaries() {
        let out = extract_identifiers("论文（https://doi.org/10.1088/1361-6560/ac1a20）。很好");
        assert_eq!(out.dois, vec!["10.1088/1361-6560/ac1a20"]);
    }

    #[test]
    fn bare_numbers_are_ignored_for_precision() {
        let out = extract_identifiers("版本号 2403.15388 和 2024.3.5 不是文献，裸 10.1/x 也不算");
        assert!(out.is_empty(), "bare forms intentionally excluded: {out:?}");
    }

    #[test]
    fn noise_urls_do_not_match() {
        let out =
            extract_identifiers("看 https://github.com/poco-ai/Agentero 和 https://www.bing.com");
        assert!(out.is_empty());
    }

    #[test]
    fn share_url_detection() {
        assert!(is_share_url(
            "https://chatgpt.com/share/6ac3cc27-d230-83ec-a2c5-7f7849d3c8e0"
        ));
        assert!(is_share_url("https://chat.openai.com/share/abc123"));
        assert!(!is_share_url("https://chatgpt.com/share/"));
        assert!(!is_share_url("https://chatgpt.com/"));
        assert!(!is_share_url("看这个 https://chatgpt.com/share/xyz 谢谢"));
        assert!(!is_share_url("https://arxiv.org/abs/2403.15388"));
        assert!(!is_share_url("http://chatgpt.com/share/abc"));
    }

    #[test]
    fn html_entity_tails_stripped() {
        // Share pages carry DOIs inside href attributes with entity-encoded
        // query tails; `&` ending identifiers cuts them without any decoding.
        let html =
            "论文 <a href=\"https://doi.org/10.1088/1361-6560/ac1a20&amp;from=chat\">链接</a>。";
        assert_eq!(
            extract_identifiers(html).dois,
            vec!["10.1088/1361-6560/ac1a20"]
        );
    }

    /// Distilled from a real chatgpt.com/share conversation: seven papers,
    /// the main one surfacing in five different forms (abs/pdf/html links,
    /// bare `arXiv:` mentions, version suffixes). The scanner must collapse
    /// them to the seven unique ids in first-seen order — this is the
    /// automated form of the accuracy sample quoted in #652.
    #[test]
    fn real_share_page_surface_forms_collapse() {
        let page = r#"
            <main>聊聊这篇：<a href="https://arxiv.org/abs/2603.29080">abs</a>
            <a href="https://arxiv.org/pdf/2603.29080">pdf</a> 和 https://arxiv.org/html/2603.29080v1
            正文里也提到 arXiv:2603.29080 与 arXiv:2603.29080v1。
            相关工作里有 <a href="https://arxiv.org/pdf/2606.10789">a</a>
            <a href="https://arxiv.org/html/2605.14893v1">b</a>
            <a href="https://arxiv.org/html/2404.07983v3">c</a>
            <a href="https://arxiv.org/abs/2605.16887">d</a>
            <a href="https://arxiv.org/abs/2405.18570">e</a>
            以及更早的 arXiv:1401.1671。
            </main>
        "#;
        assert_eq!(
            extract_identifiers(page).arxiv,
            vec![
                "2603.29080",
                "2606.10789",
                "2605.14893",
                "2404.07983",
                "2605.16887",
                "2405.18570",
                "1401.1671",
            ]
        );
    }

    #[test]
    fn identifier_edge_boundaries() {
        // URL query tails and mixed-case hosts.
        let out = extract_identifiers(
            "https://pubmed.ncbi.nlm.nih.gov/38729424/?report=classic 和 https://DOI.ORG/10.1038/nature12373",
        );
        assert_eq!(out.pmids, vec!["38729424"]);
        assert_eq!(out.dois, vec!["10.1038/nature12373"]);

        // `arXiv:` mention with a space after the colon still parses.
        assert_eq!(
            extract_identifiers("如 arXiv: 1706.03762 所示").arxiv,
            vec!["1706.03762"]
        );

        // Full-width colon after mentions (Chinese IME, credit @Fengsuid).
        let out = extract_identifiers("重点看 arXiv：2301.08745 和 PMID：31245678");
        assert_eq!(out.arxiv, vec!["2301.08745"]);
        assert_eq!(out.pmids, vec!["31245678"]);

        // The same DOI twice collapses to one entry.
        assert_eq!(
            extract_identifiers(
                "https://doi.org/10.1038/nature12373 再贴一次 doi.org/10.1038/nature12373"
            )
            .dois
            .len(),
            1
        );

        // Whitespace-only input yields nothing.
        assert!(extract_identifiers("  \n\t ").is_empty());
    }
}
