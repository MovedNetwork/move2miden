use move_binary_format::file_format::CompiledModule;

pub fn parse_module(bytes: &[u8]) -> anyhow::Result<CompiledModule> {
    let module = CompiledModule::deserialize(bytes)?;
    Ok(module)
}
