//! Write the raw converted package of a legacy file (#36 debug aid).
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    std::fs::write(&a[2], vsdx::vsd::to_package(&std::fs::read(&a[1])?)?)?;
    Ok(())
}
