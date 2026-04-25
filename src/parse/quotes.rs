use std::collections::HashMap;

pub const QUOTE_PLACEHOLDER: char = '\u{001F}';

/// First parse pass: replace each quoted region's contents with a placeholder marker
/// (`QUOTE_PLACEHOLDER` followed by an index) and return the masked string plus a dictionary
/// mapping those indices back to the original literal text. Protects string contents from later
/// tokenization in `build_tree`, which uses `resolve_literal` to look the originals back up.
pub fn mask_quotes(input: &str) -> (String, HashMap<usize, String>) {
    let mut out = String::with_capacity(input.len());
    let mut dict: HashMap<usize, String> = HashMap::new();
    let mut content = String::new();
    let mut quote: Option<char> = None;
    let mut prev = '\0';
    let mut next_index: usize = 0;
    for c in input.chars() {
        match quote {
            None => {
                out.push(c);
                if (c == '\'' || c == '"') && prev != '\\' {
                    quote = Some(c);
                    content.clear();
                }
            }
            Some(q) => {
                if c == q && prev != '\\' {
                    if !content.is_empty() {
                        out.push(QUOTE_PLACEHOLDER);
                        out.push_str(&next_index.to_string());
                        dict.insert(next_index, std::mem::take(&mut content));
                        next_index += 1;
                    }
                    out.push(c);
                    quote = None;
                } else {
                    content.push(c);
                }
            }
        }
        prev = c;
    }
    if quote.is_some() && !content.is_empty() {
        out.push(QUOTE_PLACEHOLDER);
        out.push_str(&next_index.to_string());
        dict.insert(next_index, content);
    }
    (out, dict)
}

#[cfg(test)]
mod tests {
    use super::{mask_quotes, QUOTE_PLACEHOLDER};
    use std::collections::HashMap;

    fn p(i: usize) -> String {
        format!("{}{}", QUOTE_PLACEHOLDER, i)
    }

    fn d(pairs: &[(usize, &str)]) -> HashMap<usize, String> {
        pairs.iter().map(|(i, s)| (*i, s.to_string())).collect()
    }

    #[test]
    fn test_mask_quotes() {
        let cases: Vec<(&str, String, HashMap<usize, String>)> = vec![
            ("lorem ipsum", "lorem ipsum".to_string(), d(&[])),

            ("''", "''".to_string(), d(&[])),
            ("'hello'", format!("'{}'", p(0)), d(&[(0, "hello")])),
            ("'hello''bye'", format!("'{}''{}'", p(0), p(1)), d(&[(0, "hello"), (1, "bye")])),
            ("say 'hello' to me", format!("say '{}' to me", p(0)), d(&[(0, "hello")])),
            (r"say 'hel\'lo' to me", format!("say '{}' to me", p(0)), d(&[(0, r"hel\'lo")])),
            ("say 'bye' after 'hello'.", format!("say '{}' after '{}'.", p(0), p(1)), d(&[(0, "bye"), (1, "hello")])),

            ("\"\"", "\"\"".to_string(), d(&[])),
            ("\"hello\"", format!("\"{}\"", p(0)), d(&[(0, "hello")])),
            ("\"hello\"\"bye\"", format!("\"{}\"\"{}\"", p(0), p(1)), d(&[(0, "hello"), (1, "bye")])),
            ("say \"hello\" to me", format!("say \"{}\" to me", p(0)), d(&[(0, "hello")])),
            (r#"say "hel\"lo" to me"#, format!("say \"{}\" to me", p(0)), d(&[(0, r#"hel\"lo"#)])),
            ("say \"bye\" after \"hello\".", format!("say \"{}\" after \"{}\".", p(0), p(1)), d(&[(0, "bye"), (1, "hello")])),

            (r#"say 'hey "hello" you' again"#, format!("say '{}' again", p(0)), d(&[(0, r#"hey "hello" you"#)])),
            (r#"say "hey 'hello' you" again"#, format!("say \"{}\" again", p(0)), d(&[(0, "hey 'hello' you")])),
            (r#"say 'he\'y "hello" you' again"#, format!("say '{}' again", p(0)), d(&[(0, r#"he\'y "hello" you"#)])),
            (r#"say 'he\'y "hel\"lo" you' again"#, format!("say '{}' again", p(0)), d(&[(0, r#"he\'y "hel\"lo" you"#)])),
            (r#"say "by'e" after 'hel"lo'."#, format!("say \"{}\" after '{}'.", p(0), p(1)), d(&[(0, "by'e"), (1, r#"hel"lo"#)])),
        ];

        for (input, expected_out, expected_dict) in cases {
            let (actual_out, actual_dict) = mask_quotes(input);
            assert_eq!(actual_out, expected_out, "output mismatch on: {}", input);
            assert_eq!(actual_dict, expected_dict, "dict mismatch on: {}", input);
        }
    }
}
