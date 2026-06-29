//! List discovered VST3 plugins: `cargo run -p formant-vst3 --example list_plugins`

fn main() {
    let plugins = formant_vst3::scan();
    println!("found {} VST3 plugin(s):\n", plugins.len());
    for p in &plugins {
        let kind = if p.is_instrument { "instrument" } else { "effect" };
        println!(
            "  {:<28} [{kind:^10}] {:<10} {}",
            p.name,
            p.vendor,
            p.categories.join("/")
        );
    }
}
