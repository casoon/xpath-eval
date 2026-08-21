//! Tokenizer for XPath 1.0 expressions, implementing the lexical
//! disambiguation rules of [XPath 1.0 §3.7](https://www.w3.org/TR/1999/REC-xpath-19991116/#exprlex)
//! verbatim (see `plan/02-lexer-parser.md` for the four numbered rules):
//!
//! 1. If there is a preceding token and it is not one of `@`, `::`, `(`,
//!    `[`, `,` or an `Operator`, then `*` must be recognized as
//!    `MultiplyOperator` and an `NCName` spelling `and`/`or`/`mod`/`div`
//!    must be recognized as `OperatorName` — not as `NameTest`/
//!    `FunctionName`.
//! 2. If an `NCName` is immediately followed by `(` (possibly after
//!    whitespace), it must be recognized as a `NodeType` or `FunctionName`.
//! 3. If an `NCName` is immediately followed by `::` (possibly after
//!    whitespace), it must be recognized as an `AxisName`.
//! 4. Otherwise the `NCName` is a plain `QName`/`NameTest`.
//!
//! Rule 1 needs the previously emitted token, so this lexer tracks it
//! (`last_token`) and resolves `*` and `and`/`or`/`mod`/`div` right here.
//! Rules 2 and 3 only need one token of lookahead (`(` or `::` following the
//! name) and are instead resolved by the parser once it has the full token
//! stream — the plan explicitly allows the parser to take over
//! context-sensitive disambiguation "beim Konsumieren".

use crate::ast::QName;
use crate::parser::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    DotDot,
    At,
    Comma,
    ColonColon,
    Slash,
    SlashSlash,
    Pipe,
    Plus,
    Minus,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `*` disambiguated as `MultiplyOperator` (rule 1).
    Star,
    /// `*` disambiguated as the `NameTest` wildcard (rule 1, default case).
    Wildcard,
    And,
    Or,
    Mod,
    Div,
    /// A plain `NCName` or `QName` (`prefix:local`) — interpreted by the
    /// parser as `NameTest`, `FunctionName`, `AxisName` or `NodeType` via
    /// one token of lookahead (rules 2/3).
    Name {
        prefix: Option<String>,
        local: String,
    },
    /// `prefix:*`, always unambiguously a `NameTest`.
    NsWildcard(String),
    Variable(QName),
    Literal(String),
    Number(f64),
}

pub(crate) struct Lexer<'a> {
    input: &'a str,
    chars: Vec<(usize, char)>,
    idx: usize,
    last_token: Option<Token>,
}

fn is_name_start_char(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == '.'
}

impl<'a> Lexer<'a> {
    pub(crate) fn tokenize(input: &'a str) -> Result<Vec<(Token, usize)>, ParseError> {
        let mut lexer = Lexer {
            input,
            chars: input.char_indices().collect(),
            idx: 0,
            last_token: None,
        };
        let mut tokens = Vec::new();
        loop {
            lexer.skip_whitespace();
            let start = lexer.byte_pos();
            match lexer.next_token()? {
                None => break,
                Some(tok) => {
                    lexer.last_token = Some(tok.clone());
                    tokens.push((tok, start));
                }
            }
        }
        Ok(tokens)
    }

    fn byte_pos(&self) -> usize {
        self.chars
            .get(self.idx)
            .map(|&(b, _)| b)
            .unwrap_or(self.input.len())
    }

    fn peek_char(&self) -> Option<char> {
        self.chars.get(self.idx).map(|&(_, c)| c)
    }

    fn peek_char_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.idx + offset).map(|&(_, c)| c)
    }

    fn advance_char(&mut self) -> Option<char> {
        let c = self.peek_char();
        if c.is_some() {
            self.idx += 1;
        }
        c
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek_char(), Some(' ' | '\t' | '\n' | '\r')) {
            self.idx += 1;
        }
    }

    /// Rule 1's trigger set: true when the previous token is `@`, `::`,
    /// `(`, `[`, `,`, an `Operator`, or there is no previous token at all
    /// (start of expression — the rule's antecedent "there is a preceding
    /// token" is false, so it does not force operator interpretation).
    fn expects_operand(&self) -> bool {
        match &self.last_token {
            None => true,
            Some(tok) => matches!(
                tok,
                Token::At
                    | Token::ColonColon
                    | Token::LParen
                    | Token::LBracket
                    | Token::Comma
                    | Token::Slash
                    | Token::SlashSlash
                    | Token::Pipe
                    | Token::Plus
                    | Token::Minus
                    | Token::Eq
                    | Token::Ne
                    | Token::Lt
                    | Token::Le
                    | Token::Gt
                    | Token::Ge
                    | Token::Star
                    | Token::And
                    | Token::Or
                    | Token::Mod
                    | Token::Div
            ),
        }
    }

    fn next_token(&mut self) -> Result<Option<Token>, ParseError> {
        let c = match self.peek_char() {
            None => return Ok(None),
            Some(c) => c,
        };
        let start = self.byte_pos();
        let tok = match c {
            '(' => {
                self.advance_char();
                Token::LParen
            }
            ')' => {
                self.advance_char();
                Token::RParen
            }
            '[' => {
                self.advance_char();
                Token::LBracket
            }
            ']' => {
                self.advance_char();
                Token::RBracket
            }
            ',' => {
                self.advance_char();
                Token::Comma
            }
            '@' => {
                self.advance_char();
                Token::At
            }
            '|' => {
                self.advance_char();
                Token::Pipe
            }
            '+' => {
                self.advance_char();
                Token::Plus
            }
            '-' => {
                self.advance_char();
                Token::Minus
            }
            '=' => {
                self.advance_char();
                Token::Eq
            }
            '!' => {
                self.advance_char();
                if self.peek_char() == Some('=') {
                    self.advance_char();
                    Token::Ne
                } else {
                    return Err(ParseError::new(start, "expected '=' after '!'"));
                }
            }
            '<' => {
                self.advance_char();
                if self.peek_char() == Some('=') {
                    self.advance_char();
                    Token::Le
                } else {
                    Token::Lt
                }
            }
            '>' => {
                self.advance_char();
                if self.peek_char() == Some('=') {
                    self.advance_char();
                    Token::Ge
                } else {
                    Token::Gt
                }
            }
            '/' => {
                self.advance_char();
                if self.peek_char() == Some('/') {
                    self.advance_char();
                    Token::SlashSlash
                } else {
                    Token::Slash
                }
            }
            ':' => {
                self.advance_char();
                if self.peek_char() == Some(':') {
                    self.advance_char();
                    Token::ColonColon
                } else {
                    return Err(ParseError::new(start, "unexpected character ':'"));
                }
            }
            '*' => {
                self.advance_char();
                if self.expects_operand() {
                    Token::Wildcard
                } else {
                    Token::Star
                }
            }
            '\'' | '"' => self.read_literal(c)?,
            '$' => self.read_variable()?,
            '.' => {
                if matches!(self.peek_char_at(1), Some(d) if d.is_ascii_digit()) {
                    self.read_number()
                } else {
                    self.advance_char();
                    if self.peek_char() == Some('.') {
                        self.advance_char();
                        Token::DotDot
                    } else {
                        Token::Dot
                    }
                }
            }
            d if d.is_ascii_digit() => self.read_number(),
            n if is_name_start_char(n) => self.read_name(),
            other => {
                return Err(ParseError::new(
                    start,
                    format!("unexpected character '{other}'"),
                ));
            }
        };
        Ok(Some(tok))
    }

    fn read_literal(&mut self, quote: char) -> Result<Token, ParseError> {
        let start = self.byte_pos();
        self.advance_char(); // opening quote
        let mut s = String::new();
        loop {
            match self.advance_char() {
                None => return Err(ParseError::new(start, "unterminated string literal")),
                Some(c) if c == quote => break,
                Some(c) => s.push(c),
            }
        }
        Ok(Token::Literal(s))
    }

    fn read_variable(&mut self) -> Result<Token, ParseError> {
        let start = self.byte_pos();
        self.advance_char(); // '$'
        if !matches!(self.peek_char(), Some(c) if is_name_start_char(c)) {
            return Err(ParseError::new(start, "expected a name after '$'"));
        }
        let (prefix, local) = self.read_qname_raw();
        Ok(Token::Variable(QName { prefix, local }))
    }

    /// Reads a plain `NCName` or `NCName ':' NCName` (`::` is never
    /// consumed here). Assumes the current char is a name-start char.
    fn read_qname_raw(&mut self) -> (Option<String>, String) {
        let first = self.read_ncname();
        if self.peek_char() == Some(':')
            && self.peek_char_at(1) != Some(':')
            && let Some(c) = self.peek_char_at(1)
            && is_name_start_char(c)
        {
            self.advance_char(); // ':'
            let second = self.read_ncname();
            return (Some(first), second);
        }
        (None, first)
    }

    fn read_ncname(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek_char() {
            let ok = if s.is_empty() {
                is_name_start_char(c)
            } else {
                is_name_char(c)
            };
            if !ok {
                break;
            }
            s.push(c);
            self.advance_char();
        }
        s
    }

    /// Reads a `Name` token: `NCName`, `NCName ':' NCName`, or
    /// `NCName ':' '*'` (rule handling for `::` disambiguation), applying
    /// rule 1's forced `OperatorName` interpretation where applicable.
    fn read_name(&mut self) -> Token {
        let first = self.read_ncname();
        if self.peek_char() == Some(':') && self.peek_char_at(1) != Some(':') {
            match self.peek_char_at(1) {
                Some(c) if is_name_start_char(c) => {
                    self.advance_char(); // ':'
                    let second = self.read_ncname();
                    return Token::Name {
                        prefix: Some(first),
                        local: second,
                    };
                }
                Some('*') => {
                    self.advance_char(); // ':'
                    self.advance_char(); // '*'
                    return Token::NsWildcard(first);
                }
                _ => {}
            }
        }
        self.name_or_operator(first)
    }

    fn name_or_operator(&self, text: String) -> Token {
        if !self.expects_operand() {
            match text.as_str() {
                "and" => return Token::And,
                "or" => return Token::Or,
                "mod" => return Token::Mod,
                "div" => return Token::Div,
                _ => {}
            }
        }
        Token::Name {
            prefix: None,
            local: text,
        }
    }

    fn read_number(&mut self) -> Token {
        let start_idx = self.idx;
        while matches!(self.peek_char(), Some(c) if c.is_ascii_digit()) {
            self.advance_char();
        }
        if self.peek_char() == Some('.') {
            self.advance_char();
            while matches!(self.peek_char(), Some(c) if c.is_ascii_digit()) {
                self.advance_char();
            }
        }
        let end_byte = self.byte_pos();
        let start_byte = self
            .chars
            .get(start_idx)
            .map(|&(b, _)| b)
            .unwrap_or(self.input.len());
        let text = &self.input[start_byte..end_byte];
        // The tokenizer only ever admits `Digits ('.' Digits?)?` or
        // `'.' Digits`, both of which `f64::from_str` accepts.
        let value: f64 = text.parse().expect("lexer only emits valid Number text");
        Token::Number(value)
    }
}
