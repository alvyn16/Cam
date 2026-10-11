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

/// Replace the words of one caption line with edited `text`, spreading the
/// new words over the line's original time span in proportion to their
/// length. An empty text removes the line.
pub fn edit_line(words: &mut Vec<Word>, line: &Line, text: &str) {
    let (Some(first), Some(last)) = (line.words.first(), line.words.last()) else { return };
    let Some(i0) = words.iter().position(|w| w.start == first.start && w.text == first.text) else { return };
    let Some(i1) = words.iter().rposition(|w| w.start == last.start && w.text == last.text) else { return };
    if i1 < i0 {
        return;
    }
    let (a, b) = (first.start, last.end.max(first.start));
    let new: Vec<&str> = text.split_whitespace().collect();
    let total: usize = new.iter().map(|w| w.chars().count() + 1).sum();
    let mut t = a;
    let replacement: Vec<Word> = new
        .iter()
        .map(|w| {
            let d = (b - a) * (w.chars().count() + 1) as f64 / total.max(1) as f64;
            let word = Word { start: t, end: t + d, text: (*w).to_string() };
            t += d;
            word
        })
        .collect();
    words.splice(i0..=i1, replacement);
}

pub fn line_at(lines: &[Line], t: f64) -> Option<&Line> {
    lines.iter().find(|l| t >= l.start && t <= l.end)
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    fn w(start: f64, end: f64, t: &str) -> Word {
        Word { start, end, text: t.into() }
    }

    #[test]
    fn edit_line_replaces_words_within_the_span() {
        let mut words = vec![w(0.0, 300.0, "Helo"), w(300.0, 600.0, "wrld."), w(5000.0, 5400.0, "Next")];
        let ls = lines(&words);
        assert_eq!(ls.len(), 2);
        edit_line(&mut words, &ls[0], "Hello there, world.");
        let texts: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, vec!["Hello", "there,", "world.", "Next"]);
        assert_eq!(words[0].start, 0.0);
        assert!((words[2].end - 600.0).abs() < 1e-9);
        assert!(words[1].start > words[0].start && words[2].start > words[1].start);
        let ls = lines(&words);
        edit_line(&mut words, &ls[1], "  ");
        assert_eq!(words.len(), 3);
    }
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
