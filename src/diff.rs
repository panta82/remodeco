use similar::{Algorithm, ChangeTag, TextDiff};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffTag {
    Equal,
    Delete,
    Insert,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffPart {
    pub tag: DiffTag,
    pub text: String,
}

pub fn path_diff(before: &str, after: &str) -> (Vec<DiffPart>, Vec<DiffPart>) {
    let old = tokenize(before);
    let new = tokenize(after);
    let mut config = TextDiff::configure();
    config.algorithm(Algorithm::Myers);
    let diff = config.diff_slices(&old, &new);
    let mut before_parts = Vec::new();
    let mut after_parts = Vec::new();
    for change in diff.iter_all_changes() {
        let text = (*change.value()).to_owned();
        match change.tag() {
            ChangeTag::Equal => {
                push_merged(&mut before_parts, DiffTag::Equal, &text);
                push_merged(&mut after_parts, DiffTag::Equal, &text);
            }
            ChangeTag::Delete => push_merged(&mut before_parts, DiffTag::Delete, &text),
            ChangeTag::Insert => push_merged(&mut after_parts, DiffTag::Insert, &text),
        }
    }
    (before_parts, after_parts)
}

fn tokenize(value: &str) -> Vec<&str> {
    if value.is_empty() {
        return Vec::new();
    }
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut last_class = None;
    for (index, character) in value.char_indices() {
        let class = token_class(character);
        if last_class.is_some_and(|previous| previous != class) {
            tokens.push(&value[start..index]);
            start = index;
        }
        last_class = Some(class);
    }
    tokens.push(&value[start..]);
    tokens
}

fn token_class(character: char) -> u8 {
    if character == '/' {
        0
    } else if character.is_whitespace() {
        1
    } else if matches!(character, '.' | '_' | '-') {
        2
    } else {
        3
    }
}

fn push_merged(parts: &mut Vec<DiffPart>, tag: DiffTag, text: &str) {
    if let Some(last) = parts.last_mut().filter(|part| part.tag == tag) {
        last.text.push_str(text);
    } else {
        parts.push(DiffPart {
            tag,
            text: text.to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_segments_are_highlighted_without_losing_text() {
        let (old, new) = path_diff("/music/old name.mp3", "/music/new name.mp3");
        assert_eq!(
            old.iter()
                .map(|part| part.text.as_str())
                .collect::<String>(),
            "/music/old name.mp3"
        );
        assert_eq!(
            new.iter()
                .map(|part| part.text.as_str())
                .collect::<String>(),
            "/music/new name.mp3"
        );
        assert!(
            old.iter()
                .any(|part| part.tag == DiffTag::Delete && part.text == "old")
        );
        assert!(
            new.iter()
                .any(|part| part.tag == DiffTag::Insert && part.text == "new")
        );
    }
}
