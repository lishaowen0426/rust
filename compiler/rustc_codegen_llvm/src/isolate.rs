#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(dead_code)]
use crate::llvm;
use crate::llvm::{Context, False, Module, True, Type};
use crate::ModuleLlvm;
use libc::{c_uint, c_ulonglong};
use rustc_ast::expand::allocator::{global_fn_name, ALLOCATOR_METHODS};
use rustc_hir::def_id::{CrateNum, LOCAL_CRATE};
use rustc_middle::bug;
use rustc_middle::ty::TyCtxt;
use rustc_span::sym;

static ISOLATE_STACK_INIT_FN: &'static str = "__rust_isolate_stack_init";
static ISOLATE_STACK_GLOBAL_PREFIX: &'static str = "__rust_isolate_stack";

static ISOLATE_STACK_SIZE: usize = 512 * 1024;
static ISOLATE_STACK_ALIGN: usize = 16;

pub fn isolate_stack_global_name(tcx: TyCtxt<'_>, crate_name: Option<CrateNum>) -> String {
    if let Some(cn) = crate_name {
        format!("{}_{}", ISOLATE_STACK_GLOBAL_PREFIX, tcx.crate_name(cn))
    } else {
        format!("{}_{}", ISOLATE_STACK_GLOBAL_PREFIX, tcx.crate_name(LOCAL_CRATE))
    }
}

#[instrument(level = "debug", skip(tcx, module_llvm), name = "isolate_stack_codegen")]
pub(crate) unsafe fn codegen(tcx: TyCtxt<'_>, module_llvm: &mut ModuleLlvm, module_name: &str) {
    let llcx = &*module_llvm.llcx;
    let llmod = module_llvm.llmod();

    //type
    let i8p = llvm::LLVMPointerTypeInContext(llcx, 0);
    let usize = match tcx.sess.target.pointer_width {
        16 => llvm::LLVMInt16TypeInContext(llcx),
        32 => llvm::LLVMInt32TypeInContext(llcx),
        64 => llvm::LLVMInt64TypeInContext(llcx),
        tws => bug!("Unsupported target word size for int: {}", tws),
    };

    {
        //inject static rsp
        let name = isolate_stack_global_name(tcx, None);
        let ll_g = llvm::LLVMRustGetOrInsertGlobal(llmod, name.as_ptr().cast(), name.len(), i8p);
        llvm::LLVMRustSetVisibility(ll_g, llvm::Visibility::Default);
        let init = llvm::LLVMConstNull(i8p);
        llvm::LLVMSetInitializer(ll_g, init);
    }

    {
        //create stack_init_fn
        let fn_ty =
            llvm::LLVMFunctionType(llvm::LLVMVoidTypeInContext(llcx), [].as_ptr(), 0, False);
        let init_fn = llvm::LLVMRustGetOrInsertFunction(
            llmod,
            ISOLATE_STACK_INIT_FN.as_ptr().cast(),
            ISOLATE_STACK_INIT_FN.len(),
            fn_ty,
        );

        debug!("init_fn");

        let callee = ALLOCATOR_METHODS.iter().find(|m| m.name == sym::alloc).unwrap();
        let callee_name = global_fn_name(callee.name);
        let callee_type = llvm::LLVMFunctionType(i8p, [usize, usize].as_ptr(), 2, False);
        let callee_fn = llvm::LLVMRustGetOrInsertFunction(
            llmod,
            callee_name.as_ptr().cast(),
            callee_name.len(),
            callee_type,
        );

        let llbb = llvm::LLVMAppendBasicBlockInContext(llcx, init_fn, c"entry".as_ptr().cast());

        let llbuilder = llvm::LLVMCreateBuilderInContext(llcx);
        debug!("create builder");
        llvm::LLVMPositionBuilderAtEnd(llbuilder, llbb);
        let args = [
            llvm::LLVMConstInt(usize, ISOLATE_STACK_SIZE as c_ulonglong, False),
            llvm::LLVMConstInt(usize, ISOLATE_STACK_ALIGN as c_ulonglong, False),
        ];

        let allocated = llvm::LLVMRustBuildCall(
            llbuilder,
            callee_type,
            callee_fn,
            args.as_ptr(),
            args.len() as c_uint,
            [].as_ptr(),
            0 as c_uint,
        );
        debug!("build call");

        let name = isolate_stack_global_name(tcx, None);
        let ll_g = llvm::LLVMGetNamedGlobal(llmod, name.as_ptr().cast())
            .expect("isolate stack global is not available");
        llvm::LLVMBuildStore(llbuilder, allocated, ll_g);

        llvm::LLVMBuildRetVoid(llbuilder);

        llvm::LLVMDisposeBuilder(llbuilder);
    }
}
