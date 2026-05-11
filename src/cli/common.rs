use clap::ValueEnum;

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum MatchPolicyArg {
    /// Include plans that are not currently installed.
    Missing,
    /// Include plans whose version/release differs from the installed one.
    Outdated,
    /// Include plans that are already installed and match the plan definition.
    Installed,
    /// Include all plans.
    All,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum DomainArg {
    /// Follow only ABI-sensitive link relationships.
    Link,
    /// Follow only runtime relationships.
    Runtime,
    /// Follow only build-time relationships.
    Forge,
    /// Follow all relationships (link + runtime + build).
    All,
}
