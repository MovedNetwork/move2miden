use {
    crate::{compiler, move_utils},
    anyhow::Context,
    miden::DefaultHost,
    miden_assembly::Assembler,
    move_compiler::{
        shared::{NumberFormat, NumericalAddress},
        Compiler, Flags,
    },
    std::collections::BTreeSet,
};

#[test]
fn test_compile_arithmetic() {
    let bytes = move_compile_arithmetic().unwrap();
    let move_module = move_utils::parse_module(&bytes).unwrap();
    let miden_ast = compiler::compile(&move_module).unwrap();
    let assembler = Assembler::default();
    let program = assembler.compile_ast(&miden_ast).unwrap();
    let result = miden::execute(
        &program,
        Default::default(),
        DefaultHost::default(),
        Default::default(),
    )
    .unwrap();
    let outputs = result.stack_outputs().stack();
    // Outputs are 1s for each assertion.
    assert_eq!(
        outputs,
        &[1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
}

fn move_compile_arithmetic() -> anyhow::Result<Vec<u8>> {
    let known_attributes = BTreeSet::new();
    let named_address_mapping = [(
        "arithmetic",
        NumericalAddress::new([0; 32], NumberFormat::Hex),
    )]
    .into_iter()
    .collect();
    let compiler = Compiler::from_files(
        vec!["src/tests/res/move_sources"],
        Vec::new(),
        named_address_mapping,
        Flags::empty(),
        &known_attributes,
    );
    let (_, result) = compiler
        .build()
        .context("Failed to compile arithmetic.move")?;
    let compiled_unit = result.unwrap().0.pop().unwrap().into_compiled_unit();
    let bytes = compiled_unit.serialize(None);
    Ok(bytes)
}
