pub fn solve() -> (tg_agent::tools::ToolCatalog, tg_agent::tools::ToolCatalog) {
    (
        tg_agent::tools::catalog(),
        tg_agent::tools::extended_catalog(),
    )
}
