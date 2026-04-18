use crate::kexpression::KExpression;

pub fn parse(_input: &str) -> KExpression {
    todo!()
}

pub const QUOTE_PLACEHOLDER: char = '\u{001F}';

pub fn mask_quotes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut quote: Option<char> = None;
    let mut prev = '\0';
    for c in input.chars() {
        match quote {
            None => {
                out.push(c);
                if (c == '\'' || c == '"') && prev != '\\' {
                    quote = Some(c);
                }
            }
            Some(q) => {
                if c == q && prev != '\\' {
                    out.push(c);
                    quote = None;
                } else {
                    out.push(QUOTE_PLACEHOLDER);
                }
            }
        }
        prev = c;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{mask_quotes, QUOTE_PLACEHOLDER};

    fn p(n: usize) -> String {
        QUOTE_PLACEHOLDER.to_string().repeat(n)
    }

    #[test]
    fn test_mask_quotes() {
        let cases: Vec<(&str, String)> = vec![
            ("lorem ipsum", "lorem ipsum".to_string()),

            ("''", "''".to_string()),
            ("'hello'", format!("'{}'", p(5))),
            ("'hello''bye'", format!("'{}''{}'", p(5), p(3))),
            ("say 'hello' to me", format!("say '{}' to me", p(5))),
            (r"say 'hel\'lo' to me", format!("say '{}' to me", p(7))),
            ("say 'bye' after 'hello'.", format!("say '{}' after '{}'.", p(3), p(5))),

            ("\"\"", "\"\"".to_string()),
            ("\"hello\"", format!("\"{}\"", p(5))),
            ("\"hello\"\"bye\"", format!("\"{}\"\"{}\"", p(5), p(3))),
            ("say \"hello\" to me", format!("say \"{}\" to me", p(5))),
            (r#"say "hel\"lo" to me"#, format!("say \"{}\" to me", p(7))),
            ("say \"bye\" after \"hello\".", format!("say \"{}\" after \"{}\".", p(3), p(5))),

            (r#"say 'hey "hello" you' again"#, format!("say '{}' again", p(15))),
            (r#"say "hey 'hello' you" again"#, format!("say \"{}\" again", p(15))),
            (r#"say 'he\'y "hello" you' again"#, format!("say '{}' again", p(17))),
            (r#"say 'he\'y "hel\"lo" you' again"#, format!("say '{}' again", p(19))),
            (r#"say "by'e" after 'hel"lo'."#, format!("say \"{}\" after '{}'.", p(4), p(6))),
        ];

        for (input, expected) in cases {
            let actual = mask_quotes(input);
            assert_eq!(actual, expected, "failed on: {}", input);
        }
    }
}
