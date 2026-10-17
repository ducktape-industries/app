//! `view-meter <in.wasm> <out.wasm> <view_bytes>`: the meter pass as a
//! command, for the bundled views' build (`make phone-views`) and the iOS
//! day-1 replays. A refusal is the exit status and its one line.
use std::error::Error;

/// A failure, shown as its one line (`main` prints an error's `Debug`).
struct Failed(Box<dyn Error>);

impl std::fmt::Debug for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl<E: Error + 'static> From<E> for Failed {
    fn from(error: E) -> Self {
        Self(Box::new(error))
    }
}

fn main() -> Result<(), Failed> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, view_bytes] = args.as_slice() else {
        return Err(Failed(
            "usage: view-meter <in.wasm> <out.wasm> <view_bytes>".into(),
        ));
    };
    let metered = view_meter::meter(&std::fs::read(input)?, view_bytes.parse()?)?;
    std::fs::write(output, metered)?;
    Ok(())
}
