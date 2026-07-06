//! Text chunking helpers for indexing large documents.

/// Split `text` into chunks of at most `max_chars` characters, breaking on
/// word boundaries, with roughly `overlap` characters of trailing context
/// repeated at the start of the next chunk.
///
/// Useful before embedding: retrieval works best when each chunk is a
/// self-contained passage (a few hundred to ~2000 characters, depending on
/// the embedding model).
///
/// ```
/// use corrosive_agents::vector::chunk_text;
///
/// let chunks = chunk_text(&"lorem ipsum ".repeat(100), 200, 40);
/// assert!(chunks.iter().all(|c| c.chars().count() <= 200));
/// assert!(chunks.len() > 1);
/// ```
pub fn chunk_text(text: &str, max_chars: usize, overlap: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let overlap = overlap.min(max_chars / 2);
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut current_len = 0usize;

    for word in words {
        let word_len = word.chars().count();
        let sep = usize::from(!current.is_empty());
        if current_len + sep + word_len > max_chars && !current.is_empty() {
            chunks.push(current.join(" "));
            // Seed the next chunk with ~`overlap` chars of trailing words.
            let mut carried: Vec<&str> = Vec::new();
            let mut carried_len = 0usize;
            for prev in current.iter().rev() {
                let prev_len = prev.chars().count();
                if carried_len + prev_len > overlap {
                    break;
                }
                carried_len += prev_len + 1;
                carried.push(prev);
            }
            carried.reverse();
            current = carried;
            current_len = current.iter().map(|w| w.chars().count()).sum::<usize>()
                + current.len().saturating_sub(1);
        }
        current_len += usize::from(!current.is_empty()) + word_len;
        current.push(word);
    }
    if !current.is_empty() {
        chunks.push(current.join(" "));
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_max_chars() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa".repeat(20);
        for chunk in chunk_text(&text, 80, 20) {
            assert!(chunk.chars().count() <= 80, "chunk too long: {chunk}");
        }
    }

    #[test]
    fn overlap_carries_context() {
        let text = "one two three four five six seven eight nine ten";
        let chunks = chunk_text(text, 20, 8);
        assert!(chunks.len() >= 2);
        // Some trailing words of chunk N reappear at the start of chunk N+1.
        let first_tail = chunks[0].split_whitespace().last().unwrap();
        assert!(chunks[1].contains(first_tail));
    }

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk_text("hello world", 100, 10), vec!["hello world"]);
        assert!(chunk_text("   ", 100, 10).is_empty());
    }

    #[test]
    fn single_oversized_word_still_emits() {
        let long_word = "x".repeat(50);
        let chunks = chunk_text(&long_word, 10, 2);
        assert_eq!(chunks.len(), 1);
    }
}
