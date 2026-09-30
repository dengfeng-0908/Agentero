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

/// Characters that reliably end an identifier inside chat text.
fn ident_end(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\''
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
    let hay = text.to_ascii_lowercase();

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
}
