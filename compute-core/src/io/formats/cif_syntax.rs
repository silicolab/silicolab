use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenKind {
    Bare,
    Quoted,
    Text,
}

#[derive(Debug)]
struct Token {
    value: String,
    kind: TokenKind,
    line: usize,
    column: usize,
}

impl Token {
    fn is_bare(&self, value: &str) -> bool {
        self.kind == TokenKind::Bare && self.value.eq_ignore_ascii_case(value)
    }

    fn is_tag(&self) -> bool {
        self.kind == TokenKind::Bare && self.value.starts_with('_')
    }

    fn is_block_boundary(&self) -> bool {
        self.is_tag()
            || self.is_bare("loop_")
            || self.is_bare("stop_")
            || (self.kind == TokenKind::Bare
                && (self.value.to_ascii_lowercase().starts_with("data_")
                    || self.value.to_ascii_lowercase().starts_with("save_")
                    || self.value.eq_ignore_ascii_case("global_")))
    }
}

pub(super) fn tokenize_cif(input: &str) -> Result<Vec<String>> {
    let lexical_tokens = Lexer::new(input).tokenize()?;
    structure_tokens(lexical_tokens)
}

fn structure_tokens(tokens: Vec<Token>) -> Result<Vec<String>> {
    let mut output = Vec::with_capacity(tokens.len());
    let mut index = 0;

    while index < tokens.len() {
        let token = &tokens[index];
        if token.is_bare("loop_") {
            output.push("loop_".to_string());
            index += 1;
            let header_start = index;
            while index < tokens.len() && tokens[index].is_tag() {
                output.push(tokens[index].value.clone());
                index += 1;
            }
            let width = index - header_start;
            if width == 0 {
                bail!(
                    "invalid CIF loop at line {}, column {}: missing data names",
                    token.line,
                    token.column
                );
            }

            let value_start = index;
            while index < tokens.len() && !tokens[index].is_block_boundary() {
                output.push(tokens[index].value.clone());
                index += 1;
            }
            let value_count = index - value_start;
            if value_count == 0 {
                bail!(
                    "invalid CIF loop at line {}, column {}: no values",
                    token.line,
                    token.column
                );
            }
            if value_count % width != 0 {
                bail!(
                    "invalid CIF loop at line {}, column {}: {value_count} values do not fill {width} columns",
                    token.line,
                    token.column
                );
            }
            if index < tokens.len() && tokens[index].is_bare("stop_") {
                index += 1;
            }
            continue;
        }

        if token.is_tag() {
            let value = tokens.get(index + 1).ok_or_else(|| {
                anyhow::anyhow!(
                    "invalid CIF data item `{}` at line {}, column {}: missing value",
                    token.value,
                    token.line,
                    token.column
                )
            })?;
            if value.is_block_boundary() {
                bail!(
                    "invalid CIF data item `{}` at line {}, column {}: missing value",
                    token.value,
                    token.line,
                    token.column
                );
            }
            output.push(token.value.clone());
            output.push(value.value.clone());
            index += 2;
            continue;
        }

        if token.kind == TokenKind::Bare
            && (token.value.to_ascii_lowercase().starts_with("data_")
                || token.value.to_ascii_lowercase().starts_with("save_")
                || token.value.eq_ignore_ascii_case("global_"))
        {
            output.push(token.value.clone());
            index += 1;
            continue;
        }

        bail!(
            "invalid CIF syntax at line {}, column {}: unexpected value `{}`",
            token.line,
            token.column,
            token.value
        );
    }

    Ok(output)
}

struct Lexer<'a> {
    input: &'a str,
    offset: usize,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            offset: 0,
            line: 1,
            column: 1,
        }
    }

    fn tokenize(mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        while self.peek().is_some() {
            self.skip_whitespace_and_comments();
            let Some(current) = self.peek() else {
                break;
            };
            let line = self.line;
            let column = self.column;
            let (value, kind) = match current {
                ';' if column == 1 => (self.text_field(line)?, TokenKind::Text),
                '\'' | '"' => (self.quoted(current, line, column)?, TokenKind::Quoted),
                _ => (self.bare(), TokenKind::Bare),
            };
            tokens.push(Token {
                value,
                kind,
                line,
                column,
            });
        }
        Ok(tokens)
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.advance();
            }
            if self.peek() != Some('#') {
                break;
            }
            while self.peek().is_some_and(|ch| ch != '\n' && ch != '\r') {
                self.advance();
            }
        }
    }

    fn quoted(&mut self, quote: char, line: usize, column: usize) -> Result<String> {
        self.advance();
        let start = self.offset;
        loop {
            let Some(current) = self.peek() else {
                bail!("unterminated CIF quoted value at line {line}, column {column}");
            };
            if current == quote {
                let end = self.offset;
                self.advance();
                if self.peek().is_none()
                    || self.peek().is_some_and(char::is_whitespace)
                    || self.peek() == Some('#')
                {
                    return Ok(self.input[start..end].to_string());
                }
            } else {
                self.advance();
            }
        }
    }

    fn text_field(&mut self, line: usize) -> Result<String> {
        self.advance();
        if self.peek() == Some('\r') {
            self.advance();
        }
        if self.peek() == Some('\n') {
            self.advance();
        }
        let start = self.offset;

        loop {
            let Some(current) = self.peek() else {
                bail!("unterminated CIF text field starting at line {line}");
            };
            if current == ';' && self.column == 1 {
                let mut end = self.offset;
                if self.input[..end].ends_with('\n') {
                    end -= 1;
                    if self.input[..end].ends_with('\r') {
                        end -= 1;
                    }
                } else if self.input[..end].ends_with('\r') {
                    end -= 1;
                }
                let value = self.input[start..end].to_string();
                self.advance();
                if self
                    .peek()
                    .is_some_and(|ch| !ch.is_whitespace() && ch != '#')
                {
                    bail!("invalid CIF text delimiter at line {}, column 1", self.line);
                }
                return Ok(value);
            }
            self.advance();
        }
    }

    fn bare(&mut self) -> String {
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|ch| !ch.is_whitespace() && ch != '#')
        {
            self.advance();
        }
        self.input[start..self.offset].to_string()
    }

    fn peek(&self) -> Option<char> {
        self.input[self.offset..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let current = self.peek()?;
        self.offset += current.len_utf8();
        if current == '\n' || (current == '\r' && self.peek() != Some('\n')) {
            self.line += 1;
            self.column = 1;
        } else if current != '\r' {
            self.column += 1;
        }
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::tokenize_cif;

    #[test]
    fn parses_quotes_comments_and_multiline_values() {
        let tokens = tokenize_cif(
            "data_demo\n_name 'a # quoted value' # comment\n_note\n;first\nsecond\n;\n",
        )
        .unwrap();

        assert_eq!(
            tokens,
            [
                "data_demo",
                "_name",
                "a # quoted value",
                "_note",
                "first\nsecond"
            ]
        );
    }

    #[test]
    fn accepts_control_like_quoted_loop_values() {
        let tokens = tokenize_cif("data_demo\nloop_\n_key\n_value\n'_tag' 'loop_'\n").unwrap();

        assert_eq!(
            tokens,
            ["data_demo", "loop_", "_key", "_value", "_tag", "loop_"]
        );
    }

    #[test]
    fn rejects_incomplete_loop_rows() {
        let error = tokenize_cif("data_demo\nloop_\n_x\n_y\n1 2 3\n")
            .unwrap_err()
            .to_string();

        assert!(error.contains("3 values do not fill 2 columns"));
    }

    #[test]
    fn rejects_unterminated_delimited_values() {
        assert!(tokenize_cif("data_demo\n_name 'unfinished\n").is_err());
        assert!(tokenize_cif("data_demo\n_note\n;unfinished\n").is_err());
    }
}
