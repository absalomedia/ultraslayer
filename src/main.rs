use ultraslayer::UltraSlayer;

fn main() {
    println!("⚡️ UltraSlayer: Initializing Example...");
    let slayer = UltraSlayer::<u64>::new(2, 100_000);
    slayer.insert(0, 987_654_321);
    slayer.spawn_slayer_core(1);
    let val = slayer.read(0);
    println!("Slayer Read Result: {}", val);
}

