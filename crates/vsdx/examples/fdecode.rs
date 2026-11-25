//! Score the formula decoder against `hex<TAB>Visio F` lines on stdin (#36 RE aid).
use std::io::BufRead;
fn main() {
    let (mut ok, mut none, mut wrong) = (0, 0, vec![]);
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let Some((h, f)) = line.split_once('\t') else {
            continue;
        };
        let b: Vec<u8> = h
            .split_whitespace()
            .map(|x| u8::from_str_radix(x, 16).unwrap())
            .collect();
        let b = &b[..b.len().saturating_sub(1)]; // drop 0xFE
        match vsdx::vsd::formula::decode(b, &[], u32::MAX) {
            Some(d) if d.replace(' ', "") == f.replace(' ', "") => ok += 1,
            Some(d) => wrong.push(format!("{f}  =>  {d}")),
            None => {
                none += 1;
                if none <= 40 {
                    eprintln!("UNDEC {f}\t{h}");
                }
            }
        }
    }
    println!("ok {ok}  undecoded {none}  wrong {}", wrong.len());
    for w in wrong.iter().take(25) {
        println!("  {w}");
    }
}
