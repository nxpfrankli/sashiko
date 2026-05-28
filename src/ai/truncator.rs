// Copyright 2026 The Sashiko Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::ai::token_budget::TokenBudget;
use std::ops::Range;

pub struct Truncator;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncationResult {
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequentialTruncationResult {
    pub content: String,
    pub lines_kept: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeTruncationResult {
    pub content: String,
    pub truncated: bool,
    pub start_line: usize,
    pub end_line: usize,
    pub lines_returned: usize,
}

impl Truncator {
    /// Truncates a diff output if it's too large.
    /// Preserves the header and checks for balanced chunks.
    pub fn truncate_diff(diff: &str, max_tokens: usize, label: &str) -> TruncationResult {
        let estimated = TokenBudget::estimate_tokens(diff);
        if estimated <= max_tokens {
            return TruncationResult {
                content: diff.to_string(),
                truncated: false,
            };
        }

        let max_chars = max_tokens * 4;
        let lines: Vec<&str> = diff.lines().collect();
        let total_lines = lines.len();

        // Heuristic: If total lines is small but content is huge, we have long lines.
        // We calculate 'allowed_lines' based on a conservative average line length (e.g. 50 chars).
        let allowed_lines = max_chars / 50;

        if total_lines <= allowed_lines {
            // Vulnerability Fix: If we are here, estimated > max_tokens.
            // But line count is small. This implies huge lines.
            // We must perform character-based truncation.
            let kept: String = diff.chars().take(max_chars).collect();
            return TruncationResult {
                content: format!(
                    "{}\n... [Output truncated. Content too large ({} tokens). Displaying first {} chars] ...\n",
                    kept, estimated, max_chars
                ),
                truncated: true,
            };
        }

        let keep_top = allowed_lines / 2;
        let keep_bottom = allowed_lines / 2;

        if keep_top + keep_bottom >= total_lines {
            // Should be covered by above check, but safety fallback
            let kept: String = diff.chars().take(max_chars).collect();
            return TruncationResult {
                content: format!(
                    "{}\n... [Output truncated. Content too large. Displaying first {} chars] ...\n",
                    kept, max_chars
                ),
                truncated: true,
            };
        }

        let mut result = String::new();
        for line in &lines[..keep_top] {
            result.push_str(line);
            result.push('\n');
        }

        result.push_str(&format!(
            "\n... [{} truncated. Dropped {} lines (lines {}-{})] ...\n\n",
            label,
            total_lines - (keep_top + keep_bottom),
            keep_top + 1,
            total_lines - keep_bottom
        ));

        for line in &lines[total_lines - keep_bottom..] {
            result.push_str(line);
            result.push('\n');
        }

        // Final Safety Check
        if TokenBudget::estimate_tokens(&result) > max_tokens {
            let kept: String = result.chars().take(max_chars).collect();
            return TruncationResult {
                content: format!(
                    "{}\n... [Output truncated after line filtering. Original size: {} tokens] ...\n",
                    kept, estimated
                ),
                truncated: true,
            };
        }

        TruncationResult {
            content: result,
            truncated: true,
        }
    }

    /// Sequentially truncates content, keeping only the first N lines/tokens.
    /// Appends a truncation warning.
    pub fn truncate_sequential(content: &str, max_tokens: usize) -> SequentialTruncationResult {
        let estimated = TokenBudget::estimate_tokens(content);
        if estimated <= max_tokens {
            return SequentialTruncationResult {
                content: content.to_string(),
                lines_kept: content.lines().count(),
                truncated: false,
            };
        }

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Binary search to find the maximum number of lines that fit within max_tokens
        let mut low = 0;
        let mut high = total_lines;
        let mut best_keep = 0;

        while low <= high {
            let mid = (low + high) / 2;
            let candidate = lines[..mid].join("\n");
            let cand_tokens = TokenBudget::estimate_tokens(&candidate);

            if cand_tokens <= max_tokens {
                best_keep = mid;
                low = mid + 1;
            } else {
                high = mid - 1;
            }
        }

        if best_keep == 0 {
            // Fallback to character-based truncation
            let max_chars = max_tokens * 4;
            let kept: String = content.chars().take(max_chars).collect();
            return SequentialTruncationResult {
                content: format!(
                    "{}\n... [Output truncated. Content too large ({} tokens). Displaying first {} chars] ...\n",
                    kept, estimated, max_chars
                ),
                lines_kept: 0,
                truncated: true,
            };
        }

        let mut result = lines[..best_keep].join("\n");
        result.push('\n');

        let warning = format!(
            "... [Output truncated. Dropped {} lines. Original size: {} tokens] ...\n",
            total_lines - best_keep,
            estimated
        );

        // Adjust best_keep if adding the warning pushes us over budget
        while best_keep > 0 {
            let candidate = format!("{}{}", result, warning);
            if TokenBudget::estimate_tokens(&candidate) <= max_tokens {
                return SequentialTruncationResult {
                    content: candidate,
                    lines_kept: best_keep,
                    truncated: true,
                };
            }
            best_keep -= 1;
            result = lines[..best_keep].join("\n");
            if best_keep > 0 {
                result.push('\n');
            }
        }

        let max_chars = max_tokens * 4;
        let kept: String = content.chars().take(max_chars).collect();
        SequentialTruncationResult {
            content: format!(
                "{}\n... [Output truncated. Content too large ({} tokens). Displaying first {} chars] ...\n",
                kept, estimated, max_chars
            ),
            lines_kept: 0,
            truncated: true,
        }
    }

    /// Smart truncation for code files.
    /// Tries to preserve context around `focus_lines`.
    ///
    /// `focus_lines` is 1-based inclusive range.
    pub fn truncate_code(
        content: &str,
        focus_lines: Option<Range<usize>>,
        max_tokens: usize,
    ) -> CodeTruncationResult {
        let estimated = TokenBudget::estimate_tokens(content);
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        if estimated <= max_tokens {
            return CodeTruncationResult {
                content: content.to_string(),
                truncated: false,
                start_line: 1,
                end_line: total_lines,
                lines_returned: total_lines,
            };
        }

        let (start_focus, end_focus) = if let Some(range) = focus_lines {
            let (start, end) = if range.start > range.end {
                (range.end, range.start)
            } else {
                (range.start, range.end)
            };
            (start.max(1) - 1, end.min(total_lines))
        } else {
            // If no focus, default to top part (sequential).
            let seq_res = Self::truncate_sequential(content, max_tokens);
            return CodeTruncationResult {
                content: seq_res.content,
                truncated: seq_res.truncated,
                start_line: 1,
                end_line: seq_res.lines_kept,
                lines_returned: seq_res.lines_kept,
            };
        };

        // If we have focus, we want to expand around it.
        // Let's try to include 50 lines before and after.
        let context_size = 50;
        let start_keep = start_focus.saturating_sub(context_size);
        let end_keep = (end_focus + context_size).min(total_lines);

        let mut result = String::new();

        // Add header info if we are skipping the start
        if start_keep > 0 {
            result.push_str(&format!(
                "... [{} lines collapsed (lines 1-{})] ...\n",
                start_keep, start_keep
            ));
        }

        for line in &lines[start_keep..end_keep] {
            result.push_str(line);
            result.push('\n');
        }

        if end_keep < total_lines {
            result.push_str(&format!(
                "... [{} lines collapsed (lines {}-{})] ...\n",
                total_lines - end_keep,
                end_keep + 1,
                total_lines
            ));
        }

        // Final Safety Check
        if TokenBudget::estimate_tokens(&result) > max_tokens {
            let max_chars = max_tokens * 4;
            let kept: String = result.chars().take(max_chars).collect();
            let code_lines_returned = kept.lines().filter(|l| !l.starts_with("... [")).count();
            return CodeTruncationResult {
                content: format!(
                    "{}\n... [Output truncated. Code context too large. Original size: {} tokens] ...\n",
                    kept, estimated
                ),
                truncated: true,
                start_line: start_keep + 1,
                end_line: start_keep + code_lines_returned,
                lines_returned: code_lines_returned,
            };
        }

        CodeTruncationResult {
            content: result,
            truncated: true,
            start_line: start_keep + 1,
            end_line: end_keep,
            lines_returned: end_keep - start_keep,
        }
    }

    // Future: Add AST-based collapsing here.
    #[allow(dead_code)]
    fn regex_collapse(_content: &str) -> String {
        // Placeholder for regex based function collapsing
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_diff() {
        let diff = "line1\nline2\nline3\nline4\nline5\nline6";
        // budget 5 tokens (~20 chars) < 30 chars input -> should truncate
        let res = Truncator::truncate_diff(diff, 5, "Diff");
        assert!(res.content.contains("Diff truncated"));
        assert!(res.truncated);
    }

    #[test]
    fn test_truncate_code_focus() {
        let code = (0..200)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        // Budget 300 tokens.
        // Full file ~350 tokens.
        // Collapsed (100 lines) ~175 tokens.
        // So 350 > 300 -> Collapses.
        // 175 < 300 -> Returns collapsed content.
        let res = Truncator::truncate_code(&code, Some(100..105), 300);
        // It should keep lines around 100-105.
        assert!(res.content.contains("line 100"));
        assert!(res.content.contains("line 105"));
        // It should have collapsed the start
        assert!(res.content.contains("lines collapsed"));
        assert!(res.truncated);
        assert_eq!(res.start_line, 50);
        assert_eq!(res.end_line, 155);
        assert_eq!(res.lines_returned, 106);
    }

    #[test]
    fn test_truncate_diff_long_line() {
        // 1000 chars "a", but max_tokens = 20 (approx 80 chars)
        // allowed_lines = 80/50 = 1.
        // total_lines = 1.
        // 1 <= 1 -> Triggers long line logic.
        let long_line = "a".repeat(1000);
        let res = Truncator::truncate_diff(&long_line, 20, "Diff");

        // Should strictly be around max_tokens * 4 + overhead of message
        assert!(res.content.len() < 300);
        assert!(res.content.contains("Output truncated"));
        assert!(res.content.starts_with("aaaa"));
        assert!(
            res.truncated,
            "Should be marked as truncated despite being a single line"
        );
    }

    #[test]
    fn test_truncate_sequential() {
        let content = (0..100)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let res = Truncator::truncate_sequential(&content, 50);
        assert!(res.content.contains("line 0"));
        assert!(res.content.contains("Output truncated. Dropped"));
        assert!(!res.content.contains("line 99"));
        assert!(res.truncated);
        assert!(res.lines_kept > 0);
        assert!(res.lines_kept < 100);
    }

    #[test]
    fn test_truncate_diff_precise_range() {
        let diff = (1..=20)
            .map(|i| format!("diff line {} padding text", i))
            .collect::<Vec<_>>()
            .join("\n");
        // budget 80 tokens -> allowed_lines = (80 * 4) / 50 = 6 lines.
        // keep_top = 3, keep_bottom = 3. Total 20 lines.
        // Dropped lines count: 14. Range: 4 to 17.
        let res = Truncator::truncate_diff(&diff, 80, "Diff");
        assert!(res.truncated);
        assert!(
            res.content
                .contains("Diff truncated. Dropped 14 lines (lines 4-17)")
        );
        assert!(res.content.contains("diff line 1 "));
        assert!(res.content.contains("diff line 3 "));
        assert!(res.content.contains("diff line 18 "));
        assert!(res.content.contains("diff line 20"));
    }

    #[test]
    fn test_truncate_code_no_focus_sequential() {
        let code = (1..=100)
            .map(|i| format!("code line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        // Without focus range, should do sequential truncation.
        // 40 tokens limit is plenty to keep some lines but truncate the file.
        let res = Truncator::truncate_code(&code, None, 40);
        assert!(res.truncated);
        assert_eq!(res.start_line, 1);
        assert_eq!(res.end_line, res.lines_returned);
        assert!(res.content.contains("code line 1"));
        assert!(!res.content.contains("code line 100"));
        assert!(res.content.contains("Output truncated. Dropped"));
    }

    #[test]
    fn test_truncate_code_precise_collapsed_ranges() {
        let code = (1..=200)
            .map(|i| format!("code line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        // With focus 100 to 105. Increase max_tokens to 500 to avoid triggering final safety character-cut.
        let res = Truncator::truncate_code(&code, Some(100..105), 500);
        assert!(res.truncated);
        // keep_start: focus.start (99) saturating_sub 50 = 49.
        // start_keep = 49. lines 1 to 49 collapsed.
        assert!(
            res.content
                .contains("... [49 lines collapsed (lines 1-49)] ...")
        );
        // keep_end: (105 + 50).min(200) = 155.
        // lines 156 to 200 collapsed (45 lines).
        assert!(
            res.content
                .contains("... [45 lines collapsed (lines 156-200)] ...")
        );
    }

    #[test]
    fn test_truncate_code_reverse_range() {
        let code = (1..=200)
            .map(|i| format!("code line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        // With focus 105 to 100 (reverse range)
        let start = 105;
        let end = 100;
        let res = Truncator::truncate_code(&code, Some(start..end), 500);
        assert!(res.truncated);
        // Should not panic! And it should swap 100 and 105, resulting in same behavior as 100..105
        assert!(
            res.content
                .contains("... [49 lines collapsed (lines 1-49)] ...")
        );
        assert!(
            res.content
                .contains("... [45 lines collapsed (lines 156-200)] ...")
        );
    }
}
