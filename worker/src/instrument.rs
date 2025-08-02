use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use anyhow::Result;
use tracing::info;
use wasm_encoder::{
    CodeSection, ConstExpr, DataCountSection, DataSection, DataSegment, DataSegmentMode,
    ElementMode, ElementSection, ElementSegment, Elements, EntityType, ExportSection, Function,
    FunctionSection, GlobalSection, GlobalType, ImportSection, Instruction, MemorySection, Module,
    StartSection, TableSection, TypeSection, ValType,
};
use wasmparser::{
    DataKind, ElementItems, ElementKind, ExternalKind, Parser, Payload, TableInit, TypeRef,
    VisitOperator,
};
use web_time::Instant;

pub const MODULE: &str = "wasm_ide";
pub const TICK_FN: &str = "tick_fn";
pub const GLOBAL_BLOCKS_BEFORE_TICK: &str = "blocks_before_next_tick";

trait IntoEncoderType<T> {
    fn into_encoder_type(self) -> T;
}

impl<T> IntoEncoderType<T> for T {
    fn into_encoder_type(self) -> T {
        self
    }
}

impl IntoEncoderType<wasm_encoder::BlockType> for wasmparser::BlockType {
    fn into_encoder_type(self) -> wasm_encoder::BlockType {
        match self {
            wasmparser::BlockType::Empty => wasm_encoder::BlockType::Empty,
            wasmparser::BlockType::Type(x) => {
                wasm_encoder::BlockType::Result(x.try_into().unwrap())
            }
            wasmparser::BlockType::FuncType(x) => wasm_encoder::BlockType::FunctionType(x),
        }
    }
}

impl IntoEncoderType<wasm_encoder::Catch> for wasmparser::Catch {
    fn into_encoder_type(self) -> wasm_encoder::Catch {
        match self {
            wasmparser::Catch::One { tag, label } => wasm_encoder::Catch::One { tag, label },
            wasmparser::Catch::OneRef { tag, label } => wasm_encoder::Catch::OneRef { tag, label },
            wasmparser::Catch::All { label } => wasm_encoder::Catch::All { label },
            wasmparser::Catch::AllRef { label } => wasm_encoder::Catch::AllRef { label },
        }
    }
}

impl IntoEncoderType<wasm_encoder::MemArg> for wasmparser::MemArg {
    fn into_encoder_type(self) -> wasm_encoder::MemArg {
        wasm_encoder::MemArg {
            offset: self.offset,
            align: self.align as u32,
            memory_index: self.memory,
        }
    }
}

impl IntoEncoderType<wasm_encoder::HeapType> for wasmparser::HeapType {
    fn into_encoder_type(self) -> wasm_encoder::HeapType {
        self.try_into().unwrap()
    }
}

impl IntoEncoderType<wasm_encoder::ValType> for wasmparser::ValType {
    fn into_encoder_type(self) -> wasm_encoder::ValType {
        self.try_into().unwrap()
    }
}

impl IntoEncoderType<wasm_encoder::RefType> for wasmparser::RefType {
    fn into_encoder_type(self) -> wasm_encoder::RefType {
        self.try_into().unwrap()
    }
}

impl IntoEncoderType<i128> for wasmparser::V128 {
    fn into_encoder_type(self) -> i128 {
        self.i128()
    }
}

impl IntoEncoderType<f32> for wasmparser::Ieee32 {
    fn into_encoder_type(self) -> f32 {
        f32::from_bits(self.bits())
    }
}

impl IntoEncoderType<f64> for wasmparser::Ieee64 {
    fn into_encoder_type(self) -> f64 {
        f64::from_bits(self.bits())
    }
}

macro_rules! define_operator_to_instruction_inner {
     (@exceptions TryTable { try_table: $argty:ty } => $visit:ident) => {
         fn $visit(&mut self, try_table: $argty) -> Self::Output {
             Instruction::TryTable(
                 try_table.ty.into_encoder_type(),
                 Cow::from_iter(try_table.catches.iter().map(|x| x.clone().into_encoder_type()))
             )
         }
     };
     (@mvp BrTable { targets: $argty:ty } => $visit:ident) => {
         fn $visit(&mut self, targets: $argty) -> Self::Output {
             Instruction::BrTable(
                 Cow::from_iter(targets.targets().map(|x| x.unwrap())),
                 targets.default(),
             )
         }
     };
     (@mvp CallIndirect { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self, type_index: u32, table_index: u32, _: u8) -> Self::Output {
             Instruction::CallIndirect{table: table_index, ty: type_index}
         }
     };
     (@tail_call ReturnCallIndirect { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self, type_index: u32, table_index: u32) -> Self::Output {
             Instruction::ReturnCallIndirect{table: table_index, ty: type_index}
         }
     };
     (@mvp MemorySize { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self, mem: u32, _: u8) -> Self::Output {
             Instruction::MemorySize(mem)
         }
     };
     (@mvp MemoryGrow { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self, mem: u32, _: u8) -> Self::Output {
             Instruction::MemoryGrow(mem)
         }
     };
     (@gc StructGet { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::StructGet{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc StructGetS { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::StructGetS{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc StructGetU { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::StructGetU{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc StructSet { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::StructSet{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc ArrayNewFixed { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::ArrayNewFixed{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc ArrayNewData { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::ArrayNewData{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc ArrayNewElem { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::ArrayNewElem{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc ArrayCopy { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::ArrayCopy{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc ArrayInitData { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::ArrayInitData{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc ArrayInitElem { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::ArrayInitElem{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc BrOnCast { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::BrOnCast{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@gc BrOnCastFail { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::BrOnCastFail{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@bulk_memory MemoryInit { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::MemoryInit{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@bulk_memory MemoryCopy { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::MemoryCopy{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@bulk_memory TableInit { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::TableInit{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@bulk_memory TableCopy { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::TableCopy{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@simd V128Load8Lane { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::V128Load8Lane{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@simd V128Load16Lane { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::V128Load16Lane{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@simd V128Load32Lane { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::V128Load32Lane{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@simd V128Load64Lane { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::V128Load64Lane{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@simd V128Store8Lane { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::V128Store8Lane{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@simd V128Store16Lane { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::V128Store16Lane{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@simd V128Store32Lane { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::V128Store32Lane{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@simd V128Store64Lane { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::V128Store64Lane{ $($arg: $arg.into_encoder_type(),)* }
         }
     };
     (@$proposal:ident $op:ident => $visit:ident) => {
         fn $visit(&mut self) -> Self::Output {
             Instruction::$op
         }
     };
     (@$proposal:ident $op:ident { $($arg:ident: $argty:ty),* } => $visit:ident) => {
         fn $visit(&mut self $(,$arg: $argty)*) -> Self::Output {
             Instruction::$op($($arg.into_encoder_type()),*)
         }
     };
}

macro_rules! define_operator_to_instruction {
     ($( @$proposal:ident $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident)*) => {
         $(
            define_operator_to_instruction_inner!(
                @$proposal $op $({ $($arg: $argty),* })? => $visit
            );
         )*
     }
 }

struct OperatorToInstruction();

impl<'a> wasmparser::VisitOperator<'a> for OperatorToInstruction {
    type Output = Instruction<'a>;

    wasmparser::for_each_operator!(define_operator_to_instruction);
}

pub fn instrument_binary(
    binary: &[u8],
    well_known_binary: Option<&'static str>,
) -> Result<Vec<u8>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Vec<u8>>>> = OnceLock::new();

    let start = Instant::now();

    {
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let cache = cache.lock().unwrap();

        if let Some(wkb) = well_known_binary {
            if let Some(ans) = cache.get(&wkb) {
                let elapsed = start.elapsed().as_secs_f32();
                info!("instrumentation time: {elapsed}s");
                return Ok(ans.clone());
            }
        }
    }

    let mut module = Module::new();

    let mut out_code_section = None;
    let mut remaining_code_sections = None;

    let mut tick_fn_ty = None;
    let mut global_idx = None;
    let mut num_added_imports = 0;

    for payload in Parser::new(0).parse_all(binary) {
        let payload = payload?;
        match payload {
            Payload::Version {
                num: _,
                encoding: _,
                range: _,
            }
            | Payload::End(_)
            | Payload::CustomSection(_) => {}
            Payload::CodeSectionStart {
                count,
                range: _,
                size: _,
            } => {
                remaining_code_sections = Some(count);
                out_code_section = Some(CodeSection::new());
            }
            Payload::TypeSection(types) => {
                let mut out_types = TypeSection::new();
                for ty in types {
                    for ty in ty?.into_types() {
                        out_types.subtype(&ty.try_into().unwrap());
                    }
                }
                // Add the type of the tick function, () -> ().
                tick_fn_ty = Some(out_types.len());
                out_types.function(vec![], vec![]);
                module.section(&out_types);
            }
            Payload::ImportSection(imports) => {
                let mut out_imports = ImportSection::new();
                num_added_imports += 1;
                out_imports.import(MODULE, TICK_FN, EntityType::Function(tick_fn_ty.unwrap()));
                for import in imports {
                    let import = import?;
                    let ty = match import.ty {
                        TypeRef::Func(x) => EntityType::Function(x),
                        TypeRef::Tag(x) => EntityType::Tag(x.into()),
                        TypeRef::Table(x) => EntityType::Table(x.try_into().unwrap()),
                        TypeRef::Memory(x) => EntityType::Memory(x.into()),
                        TypeRef::Global(x) => EntityType::Global(x.try_into().unwrap()),
                    };
                    out_imports.import(import.module, import.name, ty);
                }
                module.section(&out_imports);
            }
            Payload::StartSection { func, range: _ } => {
                module.section(&StartSection {
                    function_index: func + num_added_imports,
                });
            }
            Payload::FunctionSection(funcs) => {
                let mut out_funcs = FunctionSection::new();
                for func in funcs {
                    let func = func?;
                    out_funcs.function(func);
                }
                module.section(&out_funcs);
            }
            Payload::TableSection(tables) => {
                let mut out_tables = TableSection::new();
                for table in tables {
                    let table = table?;
                    if let TableInit::Expr(init) = table.init {
                        out_tables.table_with_init(table.ty.try_into().unwrap(), &init.try_into()?);
                    } else {
                        out_tables.table(table.ty.try_into().unwrap());
                    }
                }
                module.section(&out_tables);
            }
            Payload::MemorySection(memories) => {
                let mut out_memories = MemorySection::new();
                for memory in memories {
                    let memory = memory?;
                    out_memories.memory(memory.try_into()?);
                }
                module.section(&out_memories);
            }
            Payload::GlobalSection(globals) => {
                let mut out_globals = GlobalSection::new();
                for global in globals {
                    let global = global?;
                    out_globals
                        .global(global.ty.try_into().unwrap(), &global.init_expr.try_into()?);
                }
                global_idx = Some(out_globals.len());
                out_globals.global(
                    GlobalType {
                        val_type: ValType::I32,
                        mutable: true,
                        shared: false,
                    },
                    &ConstExpr::i32_const(0),
                );
                module.section(&out_globals);
            }
            Payload::ExportSection(exports) => {
                let mut out_exports = ExportSection::new();
                for export in exports {
                    let export = export?;
                    let mut idx = export.index;
                    if export.kind == ExternalKind::Func {
                        idx += num_added_imports;
                    }
                    out_exports.export(export.name, export.kind.into(), idx);
                }
                out_exports.export(
                    GLOBAL_BLOCKS_BEFORE_TICK,
                    wasm_encoder::ExportKind::Global,
                    global_idx.unwrap(),
                );
                module.section(&out_exports);
            }
            Payload::ElementSection(elements) => {
                let mut out_elements = ElementSection::new();
                for element in elements {
                    let element = element?;
                    let offset_expr_storage;
                    let mode = match element.kind {
                        ElementKind::Passive => ElementMode::Passive,
                        ElementKind::Declared => ElementMode::Declared,
                        ElementKind::Active {
                            table_index,
                            offset_expr,
                        } => {
                            offset_expr_storage = offset_expr.try_into()?;
                            ElementMode::Active {
                                table: table_index,
                                offset: &offset_expr_storage,
                            }
                        }
                    };
                    let funcs: Vec<u32>;
                    let exprs: Vec<_>;
                    let elements = match element.items {
                        ElementItems::Functions(func) => {
                            funcs = func
                                .into_iter()
                                .map(|x| x.map(|x| x + num_added_imports))
                                .collect::<Result<_, _>>()?;
                            Elements::Functions(&funcs)
                        }
                        ElementItems::Expressions(ty, ce) => {
                            exprs = ce
                                .into_iter()
                                .map(|x| Ok(x?.try_into()?))
                                .collect::<Result<_>>()?;
                            Elements::Expressions(ty.try_into().unwrap(), &exprs)
                        }
                    };
                    let segment = ElementSegment { mode, elements };
                    out_elements.segment(segment);
                }
                module.section(&out_elements);
            }
            Payload::CodeSectionEntry(fun) => {
                let count = remaining_code_sections
                    .as_mut()
                    .expect("code section not started");
                *count = count.checked_sub(1).expect("too many code sections");
                let code_section = out_code_section
                    .as_mut()
                    .expect("code section already ended");
                let locals = fun
                    .get_locals_reader()?
                    .into_iter()
                    .map(|x| {
                        let x = x?;
                        Ok((x.0, x.1.try_into().unwrap()))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut function = Function::new(locals);
                let append_tick = |function: &mut Function| {
                    let gidx = global_idx.expect("no globals section?");
                    function.instruction(&Instruction::GlobalGet(gidx));
                    function.instruction(&Instruction::I32Const(0));
                    function.instruction(&Instruction::I32Eq);
                    function.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
                    function.instruction(&Instruction::Call(0));
                    function.instruction(&Instruction::Else);
                    function.instruction(&Instruction::GlobalGet(gidx));
                    function.instruction(&Instruction::I32Const(1));
                    function.instruction(&Instruction::I32Sub);
                    function.instruction(&Instruction::GlobalSet(gidx));
                    function.instruction(&Instruction::End);
                };
                append_tick(&mut function);
                let mut op_to_insn = OperatorToInstruction();
                for op in fun.get_operators_reader()?.into_iter() {
                    let op = op?;
                    let mut insn = op_to_insn.visit_operator(&op);
                    match &mut insn {
                        Instruction::Call(x) => {
                            *x += num_added_imports;
                        }
                        Instruction::ReturnCall(x) => {
                            *x += num_added_imports;
                        }
                        Instruction::RefFunc(x) => {
                            *x += num_added_imports;
                        }
                        Instruction::Loop(_) => {
                            append_tick(&mut function);
                        }
                        _ => {}
                    };
                    function.instruction(&insn);
                }
                code_section.function(&function);
                if *count == 0 {
                    module.section(code_section);
                }
            }
            Payload::DataCountSection { count, range: _ } => {
                module.section(&DataCountSection { count });
            }
            Payload::DataSection(data) => {
                let mut out_data = DataSection::new();
                for data in data {
                    let data = data?;
                    let offset;
                    let mode = match data.kind {
                        DataKind::Passive => DataSegmentMode::Passive,
                        DataKind::Active {
                            memory_index,
                            offset_expr,
                        } => {
                            offset = offset_expr.try_into()?;
                            DataSegmentMode::Active {
                                memory_index,
                                offset: &offset,
                            }
                        }
                    };
                    out_data.segment(DataSegment {
                        mode,
                        data: data.data.iter().cloned(),
                    });
                }
                module.section(&out_data);
            }
            _ => {
                panic!("Unknown section {:?}", payload);
            }
        }
    }

    let module = module.finish();
    wasmparser::validate(&module)?;
    let elapsed = start.elapsed().as_secs_f32();
    if let Some(wkb) = well_known_binary {
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut cache = cache.lock().unwrap();
        cache.insert(wkb, module.clone());
    }
    info!("instrumentation time: {elapsed}s");
    Ok(module)
}
