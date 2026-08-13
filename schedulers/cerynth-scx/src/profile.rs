#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum Profile{
    Balanced,
    Interactive,
    Performance,
    Background,
}

impl Profile{
    pub fn slice_ns(&self) -> u64{
        match self{
            Profile::Balanced => 5_000_000,
            Profile::Interactive => 2_000_000,
            Profile::Performance => 10_000_000,
            Profile::Background => 20_000_000,
        }
    }
}