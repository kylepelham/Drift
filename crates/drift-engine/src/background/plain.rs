//! Removes terminal escape sequences (colors, cursor moves, titles) from output as it arrives. A sequence
//! split across two reads is held until the rest comes.

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const MAX_SEQUENCE: usize = 4096;

#[derive(Default)]
pub(super) struct Plain {
    /// The start of a sequence the last read ended inside.
    pending: Vec<u8>,
}

impl Plain {
    /// `bytes` without escape sequences; an unfinished one at the end waits for the next call.
    pub(super) fn strip(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut input = std::mem::take(&mut self.pending);
        input.extend_from_slice(bytes);

        let mut plain = Vec::with_capacity(input.len());
        let mut at = 0;
        while at < input.len() {
            if input[at] != ESC {
                plain.push(input[at]);
                at += 1;
                continue;
            }

            match sequence_end(&input[at..]) {
                Some(length) => at += length,
                // A sequence this long never ends; its escape is dropped and the rest reads as text.
                None if input.len() - at > MAX_SEQUENCE => at += 1,
                None => {
                    self.pending = input[at..].to_vec();
                    break;
                }
            }
        }

        plain
    }
}

/// How long the sequence at the start of `bytes` is; `None` when it has not ended yet.
fn sequence_end(bytes: &[u8]) -> Option<usize> {
    match bytes.get(1)? {
        // CSI: parameters and intermediates, then one final byte.
        b'[' => bytes[2..]
            .iter()
            .position(|byte| (0x40..=0x7e).contains(byte))
            .map(|end| end + 3),
        // OSC: ends at BEL or at ESC backslash.
        b']' => {
            let body = &bytes[2..];
            let bell = body.iter().position(|byte| *byte == BEL).map(|end| end + 3);
            let terminator = body.windows(2).position(|pair| pair == [ESC, b'\\']).map(|end| end + 4);
            [bell, terminator].into_iter().flatten().min()
        }
        // Any other escape is the escape and one byte.
        _ => Some(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_and_titles_are_removed_even_when_split_across_reads() {
        let mut plain = Plain::default();
        let colored = b"\x1b[32m\xe2\x9e\x9c\x1b[39m  Local:   \x1b[36mhttp://localhost:\x1b[1m5299\x1b[22m/\x1b[39m\n";

        assert_eq!(plain.strip(colored), "➜  Local:   http://localhost:5299/\n".as_bytes());

        assert_eq!(plain.strip(b"ready \x1b[3"), b"ready ");
        assert_eq!(plain.strip(b"6mnow\x1b[0m\n"), b"now\n");
        assert_eq!(plain.strip(b"\x1b]0;title\x07done\x1b]2;t\x1b\\!"), b"done!");
        assert_eq!(plain.strip(b"plain text"), b"plain text");
    }

    #[test]
    fn an_escape_that_never_ends_does_not_swallow_what_follows() {
        let mut plain = Plain::default();
        assert_eq!(plain.strip(b"\x1b]0;"), b"");
        let rest = vec![b'x'; MAX_SEQUENCE + 1];
        assert_eq!(plain.strip(&rest), [&b"]0;"[..], &rest].concat());
    }
}
