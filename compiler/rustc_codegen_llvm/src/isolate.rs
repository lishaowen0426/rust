#![allow(unused_variables)]
#![allow(dead_code)]
use crate::llvm;
use crate::ModuleLlvm;
use rustc_hir::def_id::{CrateNum, LOCAL_CRATE};
use rustc_middle::ty::TyCtxt;

static ISOLATE_STACK_INIT_FN: &'static str = "__rust_isolate_stack_init";
static ISOLATE_STACK_GLOBAL_PREFIX: &'static str = "__rust_isolate_stack";

pub fn isolate_stack_global_name(tcx: TyCtxt<'_>, crate_name: Option<CrateNum>) -> String {
    if let Some(cn) = crate_name {
        format!("{}_{}", ISOLATE_STACK_GLOBAL_PREFIX, tcx.crate_name(cn))
    } else {
        format!("{}_{}", ISOLATE_STACK_GLOBAL_PREFIX, tcx.crate_name(LOCAL_CRATE))
    }
}

pub(crate) unsafe fn codegen(tcx: TyCtxt<'_>, module_llvm: &mut ModuleLlvm, module_name: &str) {
    let llcx = &*module_llvm.llcx;
    let llmod = module_llvm.llmod();

    //type
    let i8p = llvm::LLVMPointerTypeInContext(llcx, 0);

    {
        //inject static rsp
        let name = isolate_stack_global_name(tcx, None);
        let ll_g = llvm::LLVMRustGetOrInsertGlobal(llmod, name.as_ptr().cast(), name.len(), i8p);
        llvm::LLVMRustSetVisibility(ll_g, llvm::Visibility::Default);
        let init = llvm::LLVMConstNull(i8p);
        llvm::LLVMSetInitializer(ll_g, init);
    }
}
