//! Captions: group transcript words into short on-screen lines.

use crate::project::recording::Word;

/// Characters per caption line before breaking.
pub const MAX_CHARS: usize = 42;
/// A pause this long starts a new line.
const PAUSE_MS: f64 = 700.0;
/// Lines closer than this stay up through the gap.
const GAP_FILL_MS: f64 = 200.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start: f64,
    pub end: f64,
    pub words: Vec<Word>,
}

impl Line {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ")
    }
}

pub fn lines(words: &[Word]) -> Vec<Line> {
    let mut out: Vec<Line> = vec![];
    for w in words.iter().filter(|w| !w.text.trim().is_empty()) {
        let start_new = match out.last() {
            None => true,
            Some(l) => {
                let len = l.text().chars().count() + 1 + w.text.trim().chars().count();
                len > MAX_CHARS || w.start - l.end > PAUSE_MS
            }
        };
        if start_new {
            out.push(Line { start: w.start, end: w.end, words: vec![w.clone()] });
        } else if let Some(l) = out.last_mut() {
            l.end = w.end;
            l.words.push(w.clone());
        }
    }
    for i in 1..out.len() {
        if out[i].start - out[i - 1].end <= GAP_FILL_MS {
            out[i - 1].end = out[i].start;
        }
    }
    out
}

pub fn line_at(lines: &[Line], t: f64) -> Option<&Line> {
    lines.iter().find(|l| t >= l.start && t <= l.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: f64, end: f64, t: &str) -> Word {
        Word { start, end, text: t.into() }
    }

    #[test]
    fn breaks_on_pause_and_length() {
        let words = [w(0.0, 200.0, "Hello"), w(250.0, 500.0, "world"), w(2000.0, 2300.0, "again")];
        let ls = lines(&words);
        assert_eq!(ls.len(), 2);
        assert_eq!(ls[0].text(), "Hello world");
        assert!(line_at(&ls, 300.0).is_some());
        assert!(line_at(&ls, 1500.0).is_none());
    }
}
