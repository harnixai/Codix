//! 编进二进制的种子与场景：`codix --mock <名字>` 不依赖工作目录。

const SEEDS: &[(&str, &str)] = &[
    ("empty", include_str!("../seeds/empty.yaml")),
    ("default", include_str!("../seeds/default.yaml")),
];

const SCENARIOS: &[(&str, &str)] = &[
    ("default", include_str!("../scenarios/default.yaml")),
    ("m21-kicked", include_str!("../scenarios/m21-kicked.yaml")),
];

pub fn seed(name: &str) -> Option<&'static str> {
    SEEDS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

pub fn scenario(name: &str) -> Option<&'static str> {
    SCENARIOS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

pub fn seed_names() -> impl Iterator<Item = &'static str> {
    SEEDS.iter().map(|(n, _)| *n).chain(std::iter::once("busy"))
}

pub fn scenario_names() -> impl Iterator<Item = &'static str> {
    SCENARIOS.iter().map(|(n, _)| *n)
}
