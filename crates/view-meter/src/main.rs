//! `view-meter <in.wasm> <out.wasm> <view_bytes>`: the meter pass as a
//! command, for the bundled views' build (`make phone-views`) and the iOS
//! day-1 replays. A refusal is the exit status and its one line.
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, view_bytes] = args.as_slice() else {
        return Err("usage: view-meter <in.wasm> <out.wasm> <view_bytes>".into());
    };
    let metered = view_meter::meter(&std::fs::read(input)?, view_bytes.parse()?)?;
    std::fs::write(output, metered)?;
    Ok(())
}
