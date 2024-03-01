use {
    crate::cfg::Cfg,
    anyhow::Error,
    miden_assembly::{
        ast::{CodeBody, Instruction, Node, ProcedureAst, ProgramAst, SourceLocation},
        ProcedureName,
    },
    move_binary_format::{
        access::ModuleAccess,
        file_format::{Bytecode, Constant, FunctionDefinition, FunctionDefinitionIndex, Signature},
        CompiledModule,
    },
};

const MAIN_NAME_REPLACEMENT: &str = "dummy_name_in_place_of_main"; // TODO: remove after name mapping

pub fn compile(module: &CompiledModule) -> anyhow::Result<ProgramAst> {
    let mut local_procs = Vec::new();
    let mut main_proc = None;
    let mut state = CompilerState::default();
    // Build up function details for compiler state
    for (index, handle) in module.function_handles().iter().enumerate() {
        let name = module.identifier_at(handle.name).to_string();
        let params = module.signature_at(handle.parameters).to_owned();
        let func_def = module.function_def_at(FunctionDefinitionIndex::new(index as u16));
        let locals = match &func_def.code {
            Some(code) => module.signature_at(code.locals).to_owned(),
            None => Signature::default(),
        };
        state.functions.push(Function {
            name,
            params,
            locals,
        });
    }
    state.constants = module.constant_pool.to_owned();
    for function in module.function_defs() {
        let mut proc = compile_function(function, &state)?;
        if function.is_entry {
            if main_proc.is_some() {
                anyhow::bail!("Cannot handle multiple entrypoints");
            }
            proc.name = ProcedureName::main();
            main_proc = Some(proc);
            // Add a dummy placeholder for main, so the local procedure indices don't shift
            local_procs.push(empty_proc(MAIN_NAME_REPLACEMENT.into())?);
        } else {
            local_procs.push(proc);
        }
    }
    let main_proc = main_proc.ok_or_else(|| Error::msg("No entry point defined"))?;
    let result = ProgramAst::new(main_proc.body.nodes().to_vec(), local_procs)?;
    Ok(result)
}

/// Struct definition of a module function.
#[derive(Debug, Default)]
struct Function {
    name: String,
    params: Signature,
    locals: Signature,
}

/// Struct carrying extra information needed during compilation.
#[derive(Debug, Default)]
struct CompilerState {
    constants: Vec<Constant>,
    functions: Vec<Function>,
}

fn compile_function(
    func_def: &FunctionDefinition,
    state: &CompilerState,
) -> anyhow::Result<ProcedureAst> {
    let function = state
        .functions
        .get(func_def.function.0 as usize)
        .ok_or_else(|| Error::msg("Missing function handle index"))?;
    let code = match &func_def.code {
        Some(code) => code,
        None => return empty_proc(function.name.clone()),
    };
    let _locals = &function.locals;
    let _cfg = Cfg::new(&code.code)?; // TODO: use cfg to handle control flow
    let nodes = compile_body(&code.code, state)?;
    let body = CodeBody::new(nodes);
    let result = ProcedureAst {
        name: function.name.as_str().try_into().map_err(Error::msg)?,
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
            Bytecode::Pop => Node::Instruction(Instruction::Drop), // TODO: type validation
            Bytecode::MoveLoc(_) => continue,                      // TODO: properly handle locals
            Bytecode::Ret => continue, // TODO: properly handle function return
            Bytecode::Abort => Node::Instruction(Instruction::Drop), // TODO: type validation, stack emptiness
            Bytecode::BrFalse(_) => continue, // TODO: properly handle control flow
            Bytecode::Branch(_) => continue,  // TODO: properly handle control flow
            Bytecode::Call(index) => {
                let _name = &state
                    .functions
                    .get(index.0 as usize)
                    .ok_or_else(|| Error::msg("Missing function handle index"))?
                    .name;
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

fn empty_proc(name: String) -> anyhow::Result<ProcedureAst> {
    let name = name
        .as_str()
        .try_into()
        .map_err(|e| anyhow::anyhow!("Failed to parse function name: {e:?}"))?;
    Ok(ProcedureAst {
        name,
        docs: None,
        num_locals: 0,
        body: CodeBody::new(Vec::new()),
        start: SourceLocation::default(),
        is_export: false,
    })
}
