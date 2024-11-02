#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(dead_code)]
use crate::base::iter_globals;
use crate::llvm::{self, LLVMAppendToUsed};
use crate::llvm::{Context, False, Module, True, Type};
use crate::to_llvm_tls_model;
use crate::ModuleLlvm;
use libc::{c_uint, c_ulonglong};
use rustc_ast::expand::allocator::{global_fn_name, ALLOCATOR_METHODS};
use rustc_codegen_ssa::isolate_symbols::{
    isolate_stack_fn_name, isolate_stack_global_name, ISOLATE_STACK_ALIGN, ISOLATE_STACK_SIZE,
};
use rustc_hir::def_id::{CrateNum, LOCAL_CRATE};
use rustc_middle::bug;
use rustc_middle::ty::TyCtxt;
use rustc_span::sym;
use std::str::from_utf8;

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
        let tlm = tcx.sess.tls_model();
        llvm::LLVMSetThreadLocalMode(ll_g, to_llvm_tls_model(tlm));
        llvm::LLVMSetGlobalConstant(ll_g, False);
        let init = llvm::LLVMConstNull(i8p);
        llvm::LLVMSetInitializer(ll_g, init);

        llvm::LLVMAppendToUsed(llmod, ll_g);
    }

    {
        //create stack_init_fn
        let fn_name = isolate_stack_fn_name(tcx, None);
        let fn_ty =
            llvm::LLVMFunctionType(llvm::LLVMVoidTypeInContext(llcx), [].as_ptr(), 0, False);
        let init_fn =
            llvm::LLVMRustGetOrInsertFunction(llmod, fn_name.as_ptr().cast(), fn_name.len(), fn_ty);
        llvm::LLVMRustSetVisibility(init_fn, llvm::Visibility::Default);

        llvm::LLVMAppendGlobalCtor(llmod, init_fn);

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

        /*
        {
            for v in iter_globals(llmod) {
                debug!("global name: {}", from_utf8(llvm::get_value_name(v)).unwrap());
            }
        }
        */

        let name = isolate_stack_global_name(tcx, None);

        let ll_g = llvm::LLVMRustGetOrInsertGlobal(llmod, name.as_ptr().cast(), name.len(), i8p);
        llvm::LLVMBuildStore(llbuilder, allocated, ll_g);
        /*
        if let Some(ll_g) = llvm::LLVMGetNamedGlobal(llmod, name.as_ptr().cast()) {
            llvm::LLVMBuildStore(llbuilder, allocated, ll_g);
        } else {
            for v in iter_globals(llmod) {
                let n = from_utf8(llvm::get_value_name(v)).unwrap();
                println!("global name: {}, bytes:{:?}", n, n.as_bytes());
            }
            panic!("cannot find isolate stack:{}", name);
        }
        */
        //.expect(format!("isolate stack global is not available :{}", name).as_str());

        {
            //debug
            let tp =
                llvm::LLVMFunctionType(llvm::LLVMVoidTypeInContext(llcx), [].as_ptr(), 0, False);
            let f = llvm::LLVMRustGetOrInsertFunction(
                llmod,
                "try_to_print".as_ptr().cast(),
                "try_to_print".len(),
                tp,
            );
            let _ = llvm::LLVMRustBuildCall(
                llbuilder,
                tp,
                f,
                [].as_ptr(),
                0 as c_uint,
                [].as_ptr(),
                0 as c_uint,
            );
        }

        llvm::LLVMBuildRetVoid(llbuilder);

        llvm::LLVMDisposeBuilder(llbuilder);
    }
}
