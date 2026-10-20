//! Syntax colors with syntect's grammars. Scopes are turned into a few
//! token kinds, so the colors come from the theme.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::theme::Token;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// Colored runs of one line: (start byte, end byte, token).
pub type LineTokens = Vec<(u32, u32, Token)>;

fn syntax_for(path: &Path) -> Option<&'static SyntaxReference> {
    let set = &*SYNTAXES;
    let name = path.file_name()?.to_str()?;
    let by_name = |n: &str| set.find_syntax_by_name(n);
    if name.ends_with(".erb") {
        return by_name("HTML (Rails)").or_else(|| by_name("HTML"));
    }
    if matches!(name, "Gemfile" | "Rakefile" | "Guardfile" | "Capfile" | "Gemfile.lock") {
        return by_name("Ruby");
    }
    let ext = path.extension()?.to_str()?;
    let ext = match ext {
        "rake" | "ru" | "gemspec" | "jbuilder" | "rbs" => "rb",
        "ts" | "tsx" | "jsx" | "mjs" | "cjs" => "js",
        "scss" => "css",
        "yaml" => "yml",
        other => other,
    };
    set.find_syntax_by_extension(ext)
}

fn classify(scope: Scope) -> Option<Token> {
    let s = scope.build_string();
    let starts = |p: &str| s.starts_with(p);
    Some(if starts("comment") {
        Token::Comment
    } else if starts("string") {
        Token::String
    } else if starts("constant.other.symbol") || starts("constant.language.symbol") {
        Token::Symbol
    } else if starts("constant.numeric") {
        Token::Number
    } else if starts("constant.language") || starts("keyword") || starts("storage") {
        Token::Keyword
    } else if starts("variable.other.readwrite.instance")
        || starts("variable.other.readwrite.class")
        || starts("variable.other.readwrite.global")
    {
        Token::Ivar
    } else if starts("variable.parameter") {
        Token::Param
    } else if starts("entity.name.function") || starts("support.function") {
        Token::Method
    } else if starts("entity.name.type")
        || starts("entity.name.class")
        || starts("entity.name.module")
        || starts("entity.other.inherited-class")
        || starts("support.class")
        || starts("variable.other.constant")
        || starts("support.constant")
    {
        Token::Constant
    } else if starts("entity.name.tag") {
        Token::Tag
    } else if starts("entity.other.attribute-name") {
        Token::Attribute
    } else if starts("punctuation") {
        Token::Punctuation
    } else {
        return None;
    })
}

struct Classifier {
    cache: HashMap<Scope, Option<Token>>,
}

impl Classifier {
    fn token(&mut self, stack: &ScopeStack) -> Token {
        let scopes = stack.as_slice();
        // A string or a comment colors everything inside it.
        for s in scopes {
            if let Some(t @ (Token::String | Token::Comment)) = *self.cache.entry(*s).or_insert_with(|| classify(*s)) {
                return t;
            }
        }
        for s in scopes.iter().rev() {
            if let Some(t) = *self.cache.entry(*s).or_insert_with(|| classify(*s)) {
                return t;
            }
        }
        Token::Plain
    }
}

/// Colors every line of a file. Returns `None` for files with no grammar.
pub fn highlight(path: &Path, text: &str) -> Option<Vec<LineTokens>> {
    let syntax = syntax_for(path)?;
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut classifier = Classifier { cache: HashMap::new() };
    let mut out = Vec::new();
    for line in text.split_inclusive('\n') {
        // Very long lines (minified files) are not worth coloring.
        if line.len() > 4000 {
            out.push(Vec::new());
            continue;
        }
        let ops = state.parse_line(line, &SYNTAXES).ok()?;
        let mut runs: LineTokens = Vec::new();
        let mut pos = 0usize;
        let content_len = line.trim_end_matches(['\n', '\r']).len();
        let push = |start: usize, end: usize, token: Token, runs: &mut LineTokens| {
            let end = end.min(content_len);
            if start >= end {
                return;
            }
            if let Some(last) = runs.last_mut()
                && last.2 == token
                && last.1 as usize == start
            {
                last.1 = end as u32;
                return;
            }
            runs.push((start as u32, end as u32, token));
        };
        for (at, op) in ops {
            if at > pos {
                let t = classifier.token(&stack);
                push(pos, at, t, &mut runs);
                pos = at;
            }
            stack.apply(&op).ok()?;
        }
        if pos < line.len() {
            let t = classifier.token(&stack);
            push(pos, line.len(), t, &mut runs);
        }
        out.push(runs);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_ruby() {
        let lines = highlight(Path::new("a.rb"), "class Foo\n  # note\n  X = \"x\"\nend\n").unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].iter().any(|(_, _, t)| *t == Token::Keyword));
        assert!(lines[1].iter().any(|(_, _, t)| *t == Token::Comment));
        assert!(lines[2].iter().any(|(_, _, t)| *t == Token::String));
    }
}
