use {
    miden_assembly::{
        ast::{CodeBody, Instruction, Node, ProcedureAst, ProgramAst, SourceLocation},
        ProcedureName,
    },
    move_binary_format::{
        access::ModuleAccess,
        file_format::{Bytecode, FunctionDefinition, Signature},
        CompiledModule,
    },
};

const MAIN_NAME_REPLACEMENT: &str = "dummy_name_in_place_of_main";

pub fn compile(module: &CompiledModule) -> anyhow::Result<ProgramAst> {
    let mut local_procs = Vec::new();
    let mut main_proc = None;
    let mut state = CompilerState::default();
    for handle in module.function_handles() {
        let name_index = handle.name;
        let name = module.identifier_at(name_index);
        state.function_names.push(name.to_string())
    }
    for signature in module.signatures() {
        state.function_signatures.push(signature.clone());
    }
    for function in module.function_defs() {
        let mut proc = compile_function(function, &state)?;
        if function.is_entry {
            if main_proc.is_some() {
                anyhow::bail!("Cannot handle multiple entrypoints");
            }
            proc.name = ProcedureName::main();
            main_proc = Some(proc);
            // Add a dummy placeholder for main, so the local procedure indices don't shift
            local_procs.push(empty_proc(
                MAIN_NAME_REPLACEMENT
                    .try_into()
                    .map_err(anyhow::Error::msg)?,
            ));
        } else {
            local_procs.push(proc);
        }
    }
    let main_proc = main_proc.ok_or_else(|| anyhow::Error::msg("No entry point defined"))?;
    let result = ProgramAst::new(main_proc.body.nodes().to_vec(), local_procs)?;
    Ok(result)
}

/// Struct carrying extra information needed during compilation.
#[derive(Debug, Default)]
struct CompilerState {
    function_names: Vec<String>,
    function_signatures: Vec<Signature>,
}

fn compile_function(
    function: &FunctionDefinition,
    state: &CompilerState,
) -> anyhow::Result<ProcedureAst> {
    let name = state
        .function_names
        .get(function.function.0 as usize)
        .ok_or_else(|| anyhow::Error::msg("Missing function handle index"))?;
    let name = name
        .as_str()
        .try_into()
        .map_err(|e| anyhow::anyhow!("Failed to parse function name: {e:?}"))?;
    let code = match &function.code {
        Some(code) => code,
        None => return Ok(empty_proc(name)),
    };
    let _locals = state
        .function_signatures
        .get(code.locals.0 as usize)
        .ok_or_else(|| anyhow::Error::msg("Missing signature index"))?;
    let nodes = compile_body(&code.code, state)?;
    let body = CodeBody::new(nodes);
    let result = ProcedureAst {
        name,
        docs: None,
        num_locals: 0, // TODO: use `locals` from function definition
        body,
        start: SourceLocation::default(),
        is_export: false,
    };
    Ok(result)
}

fn compile_body(bytecode: &[Bytecode], state: &CompilerState) -> anyhow::Result<Vec<Node>> {
    let mut result = Vec::new();
    for c in bytecode {
        let node = match c {
            Bytecode::Add => Node::Instruction(Instruction::Add),
            Bytecode::Sub => Node::Instruction(Instruction::Sub),
            Bytecode::Mul => Node::Instruction(Instruction::Mul),
            Bytecode::Div => Node::Instruction(Instruction::U32CheckedDiv),
            Bytecode::Mod => Node::Instruction(Instruction::U32CheckedMod),
            Bytecode::LdU32(x) => Node::Instruction(Instruction::PushU32(*x)),
            Bytecode::LdU64(x) => {
                let x = *x;
                if x <= u32::MAX as u64 {
                    Node::Instruction(Instruction::PushU32(x as u32))
                } else {
                    // TODO: handle u64 numbers
                    anyhow::bail!("Can't handle u64 numbers yet");
                }
            }
            Bytecode::Eq => Node::Instruction(Instruction::Eq),
            Bytecode::Pop => continue,
            Bytecode::MoveLoc(_) => continue, // TODO: properly handle locals
            Bytecode::Ret => continue,        // TODO: properly handle function return
            Bytecode::Abort => continue,      // TODO: properly handle aborts
            Bytecode::BrFalse(_) => continue, // TODO: properly handle control flow
            Bytecode::Branch(_) => continue,  // TODO: properly handle control flow
            Bytecode::Call(index) => {
                let _name = state
                    .function_names
                    .get(index.0 as usize)
                    .ok_or_else(|| anyhow::Error::msg("Missing function handle index"))?;
                // TODO: use the name to figure out what to call.
                Node::Instruction(Instruction::ExecLocal(index.0))
            }
            // TODO: other bytecodes
            _ => anyhow::bail!("Unimplemented opcode {c:?}"),
        };
        result.push(node);
    }
    Ok(result)
}

fn empty_proc(name: ProcedureName) -> ProcedureAst {
    ProcedureAst {
        name,
        docs: None,
        num_locals: 0,
        body: CodeBody::new(Vec::new()),
        start: SourceLocation::default(),
        is_export: false,
    }
}
