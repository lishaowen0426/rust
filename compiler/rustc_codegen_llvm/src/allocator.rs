#![allow(unused_imports)]
use crate::attributes;
use libc::c_uint;
use rustc_ast::expand::allocator::{
    alloc_error_handler_name, default_fn_name, global_fn_name, mimalloc_fn_name, AllocatorKind,
    AllocatorTy, ALLOCATOR_METHODS, ALLOC_IS_UNSAFE, NO_ALLOC_SHIM_IS_UNSTABLE,
};
use rustc_middle::bug;
use rustc_middle::ty::TyCtxt;
use rustc_session::config::{DebugInfo, OomStrategy};
use rustc_span::symbol::{sym, Symbol};

use crate::debuginfo;
use crate::llvm::{self, Context, False, Module, True, Type};
use crate::ModuleLlvm;

const GET_UNSAFE_ALLOC_FLAG_FUNC: &str = "__get_alloc_is_unsafe";
const SET_UNSAFE_ALLOC_FLAG_FUNC: &str = "__set_alloc_is_unsafe";

#[instrument(level = "debug", skip(tcx, module_llvm))]
pub(crate) unsafe fn codegen(
    tcx: TyCtxt<'_>,
    module_llvm: &mut ModuleLlvm,
    module_name: &str,
    kind: AllocatorKind,
    alloc_error_handler_kind: AllocatorKind,
) {
    let llcx = &*module_llvm.llcx;
    let llmod = module_llvm.llmod();
    let usize = match tcx.sess.target.pointer_width {
        16 => llvm::LLVMInt16TypeInContext(llcx),
        32 => llvm::LLVMInt32TypeInContext(llcx),
        64 => llvm::LLVMInt64TypeInContext(llcx),
        tws => bug!("Unsupported target word size for int: {}", tws),
    };
    let i8 = llvm::LLVMInt8TypeInContext(llcx);
    let i8p = llvm::LLVMPointerTypeInContext(llcx, 0);

    {
        // allocate global flag for unsafe allocation
        let name = ALLOC_IS_UNSAFE;
        let ll_g = llvm::LLVMRustGetOrInsertGlobal(llmod, name.as_ptr().cast(), name.len(), i8);
        if tcx.sess.default_hidden_visibility() {
            llvm::LLVMRustSetVisibility(ll_g, llvm::Visibility::Hidden);
        }
        llvm::LLVMRustSetGlobalConst(ll_g, 0);
        let llval = llvm::LLVMConstInt(i8, 0, False);
        llvm::LLVMSetInitializer(ll_g, llval);
    }
    {
        // create set/get functions for the global flag
        create_alloc_unsafe_flag_function(tcx, llcx, llmod);
    }

    // in dev, we first test only the alloc_unsafe function
    // to check if we can generate correct llvm IR

    let is_alloc_func = |s: Symbol| {
        return s == sym::alloc || s == sym::realloc || s == sym::alloc_zeroed;
    };

    if kind == AllocatorKind::Default {
        for method in ALLOCATOR_METHODS {
            let mut args = Vec::with_capacity(method.inputs.len());
            for input in method.inputs.iter() {
                match input.ty {
                    AllocatorTy::Layout => {
                        args.push(usize); // size
                        args.push(usize); // align
                    }
                    AllocatorTy::Ptr => args.push(i8p),
                    AllocatorTy::Usize => args.push(usize),

                    AllocatorTy::ResultPtr | AllocatorTy::Unit => panic!("invalid allocator arg"),
                }
            }
            let output = match method.output {
                AllocatorTy::ResultPtr => Some(i8p),
                AllocatorTy::Unit => None,

                AllocatorTy::Layout | AllocatorTy::Usize | AllocatorTy::Ptr => {
                    panic!("invalid allocator output")
                }
            };

            let from_name = global_fn_name(method.name);
            let to_name = default_fn_name(method.name);
            //let to_name = mimalloc_fn_name(method.name);

            if is_alloc_func(method.name) {
                create_wrapper_function_with_unsafe_flag(
                    tcx, llcx, llmod, &from_name, &to_name, &args, output, false,
                );
            } else {
                create_wrapper_function(
                    tcx, llcx, llmod, &from_name, &to_name, &args, output, false,
                );
            }
        }
    }

    // rust alloc error handler
    create_wrapper_function(
        tcx,
        llcx,
        llmod,
        "__rust_alloc_error_handler",
        alloc_error_handler_name(alloc_error_handler_kind),
        &[usize, usize], // size, align
        None,
        true,
    );

    // __rust_alloc_error_handler_should_panic
    let name = OomStrategy::SYMBOL;
    let ll_g = llvm::LLVMRustGetOrInsertGlobal(llmod, name.as_ptr().cast(), name.len(), i8);
    if tcx.sess.default_hidden_visibility() {
        llvm::LLVMRustSetVisibility(ll_g, llvm::Visibility::Hidden);
    }
    let val = tcx.sess.opts.unstable_opts.oom.should_panic();
    let llval = llvm::LLVMConstInt(i8, val as u64, False);
    llvm::LLVMSetInitializer(ll_g, llval);
    {
        let name = NO_ALLOC_SHIM_IS_UNSTABLE;
        let ll_g = llvm::LLVMRustGetOrInsertGlobal(llmod, name.as_ptr().cast(), name.len(), i8);
        if tcx.sess.default_hidden_visibility() {
            llvm::LLVMRustSetVisibility(ll_g, llvm::Visibility::Hidden);
        }
        let llval = llvm::LLVMConstInt(i8, 0, False);
        llvm::LLVMSetInitializer(ll_g, llval);
    }

    if tcx.sess.opts.debuginfo != DebugInfo::None {
        let dbg_cx = debuginfo::CodegenUnitDebugContext::new(llmod);
        debuginfo::metadata::build_compile_unit_di_node(tcx, module_name, &dbg_cx);
        dbg_cx.finalize(tcx.sess);
    }
}

#[instrument(level = "debug", skip(tcx, llcx, llmod, args, output, no_return))]
fn create_wrapper_function(
    tcx: TyCtxt<'_>,
    llcx: &Context,
    llmod: &Module,
    from_name: &str,
    to_name: &str,
    args: &[&Type],
    output: Option<&Type>,
    no_return: bool,
) {
    unsafe {
        let ty = llvm::LLVMFunctionType(
            output.unwrap_or_else(|| llvm::LLVMVoidTypeInContext(llcx)),
            args.as_ptr(),
            args.len() as c_uint,
            False,
        );
        let llfn = llvm::LLVMRustGetOrInsertFunction(
            llmod,
            from_name.as_ptr().cast(),
            from_name.len(),
            ty,
        );
        let no_return = if no_return {
            // -> ! DIFlagNoReturn
            let no_return = llvm::AttributeKind::NoReturn.create_attr(llcx);
            attributes::apply_to_llfn(llfn, llvm::AttributePlace::Function, &[no_return]);
            Some(no_return)
        } else {
            None
        };

        if tcx.sess.default_hidden_visibility() {
            llvm::LLVMRustSetVisibility(llfn, llvm::Visibility::Hidden);
        }
        if tcx.sess.must_emit_unwind_tables() {
            let uwtable =
                attributes::uwtable_attr(llcx, tcx.sess.opts.unstable_opts.use_sync_unwind);
            attributes::apply_to_llfn(llfn, llvm::AttributePlace::Function, &[uwtable]);
        }

        let callee =
            llvm::LLVMRustGetOrInsertFunction(llmod, to_name.as_ptr().cast(), to_name.len(), ty);
        if let Some(no_return) = no_return {
            // -> ! DIFlagNoReturn
            attributes::apply_to_llfn(callee, llvm::AttributePlace::Function, &[no_return]);
        }
        llvm::LLVMRustSetVisibility(callee, llvm::Visibility::Hidden);

        let llbb = llvm::LLVMAppendBasicBlockInContext(llcx, llfn, c"entry".as_ptr().cast());

        let llbuilder = llvm::LLVMCreateBuilderInContext(llcx);
        llvm::LLVMPositionBuilderAtEnd(llbuilder, llbb);

        let args = args
            .iter()
            .enumerate()
            .map(|(i, _)| llvm::LLVMGetParam(llfn, i as c_uint))
            .collect::<Vec<_>>();
        let ret = llvm::LLVMRustBuildCall(
            llbuilder,
            ty,
            callee,
            args.as_ptr(),
            args.len() as c_uint,
            [].as_ptr(),
            0 as c_uint,
            0 as c_uint,
        );
        llvm::LLVMSetTailCall(ret, True);
        if output.is_some() {
            llvm::LLVMBuildRet(llbuilder, ret);
        } else {
            llvm::LLVMBuildRetVoid(llbuilder);
        }

        llvm::LLVMDisposeBuilder(llbuilder);
    }
}
#[instrument(level = "debug", skip(tcx, llcx, llmod, args, output, no_return))]

fn create_wrapper_function_with_unsafe_flag(
    tcx: TyCtxt<'_>,
    llcx: &Context,
    llmod: &Module,
    from_name: &str,
    to_name: &str,
    args: &[&Type],
    output: Option<&Type>,
    no_return: bool,
) {
    unsafe {
        let ty = llvm::LLVMFunctionType(
            output.unwrap_or_else(|| llvm::LLVMVoidTypeInContext(llcx)),
            args.as_ptr(),
            args.len() as c_uint,
            False,
        );

        /*
            pub unsafe extern "C" fn __rdl_alloc_unsafe(
            size: usize,
            align: usize,
            is_unsafe: i8,
        ) -> *mut u8 ;
             */
        let flag_ty = llvm::LLVMInt8TypeInContext(llcx);
        let allocator_callee_arg_types = [args, &[&flag_ty]].concat();
        let allocator_callee_ty = llvm::LLVMFunctionType(
            output.unwrap_or_else(|| llvm::LLVMVoidTypeInContext(llcx)),
            allocator_callee_arg_types.as_ptr(),
            allocator_callee_arg_types.len() as c_uint,
            False,
        );

        let get_flag_callee_arg_types = vec![];
        let get_flag_callee_ty = llvm::LLVMFunctionType(
            flag_ty,
            get_flag_callee_arg_types.as_ptr(),
            get_flag_callee_arg_types.len() as c_uint,
            False,
        );
        let get_flag_callee_fn = llvm::LLVMRustGetOrInsertFunction(
            llmod,
            GET_UNSAFE_ALLOC_FLAG_FUNC.as_ptr().cast(),
            GET_UNSAFE_ALLOC_FLAG_FUNC.len(),
            get_flag_callee_ty,
        );

        let llfn = llvm::LLVMRustGetOrInsertFunction(
            llmod,
            from_name.as_ptr().cast(),
            from_name.len(),
            ty,
        );
        let no_return = if no_return {
            // -> ! DIFlagNoReturn
            let no_return = llvm::AttributeKind::NoReturn.create_attr(llcx);
            attributes::apply_to_llfn(llfn, llvm::AttributePlace::Function, &[no_return]);
            Some(no_return)
        } else {
            None
        };

        if tcx.sess.default_hidden_visibility() {
            llvm::LLVMRustSetVisibility(llfn, llvm::Visibility::Hidden);
        }
        if tcx.sess.must_emit_unwind_tables() {
            let uwtable =
                attributes::uwtable_attr(llcx, tcx.sess.opts.unstable_opts.use_sync_unwind);
            attributes::apply_to_llfn(llfn, llvm::AttributePlace::Function, &[uwtable]);
        }

        let callee = llvm::LLVMRustGetOrInsertFunction(
            llmod,
            to_name.as_ptr().cast(),
            to_name.len(),
            allocator_callee_ty,
        );
        if let Some(no_return) = no_return {
            // -> ! DIFlagNoReturn
            attributes::apply_to_llfn(callee, llvm::AttributePlace::Function, &[no_return]);
        }
        llvm::LLVMRustSetVisibility(callee, llvm::Visibility::Hidden);

        let llbb = llvm::LLVMAppendBasicBlockInContext(llcx, llfn, c"entry".as_ptr().cast());

        let llbuilder = llvm::LLVMCreateBuilderInContext(llcx);
        llvm::LLVMPositionBuilderAtEnd(llbuilder, llbb);

        // get flag
        let flag_value = llvm::LLVMRustBuildCall(
            &llbuilder,
            get_flag_callee_ty,
            get_flag_callee_fn,
            [].as_ptr(),
            0 as c_uint,
            [].as_ptr(),
            0 as c_uint,
            False,
        );

        let mut args = args
            .iter()
            .enumerate()
            .map(|(i, _)| llvm::LLVMGetParam(llfn, i as c_uint))
            .collect::<Vec<_>>();
        args.push(flag_value);

        let ret = llvm::LLVMRustBuildCall(
            llbuilder,
            allocator_callee_ty,
            callee,
            args.as_ptr(),
            args.len() as c_uint,
            [].as_ptr(),
            0 as c_uint,
            0 as c_uint,
        );
        llvm::LLVMSetTailCall(ret, True);
        if output.is_some() {
            llvm::LLVMBuildRet(llbuilder, ret);
        } else {
            llvm::LLVMBuildRetVoid(llbuilder);
        }

        llvm::LLVMDisposeBuilder(llbuilder);
    }
}

#[instrument(level = "debug", skip(tcx, llcx, llmod))]
fn create_alloc_unsafe_flag_function(tcx: TyCtxt<'_>, llcx: &Context, llmod: &Module) {
    //get
    unsafe {
        let i8 = llvm::LLVMInt8TypeInContext(llcx);
        let args = Vec::new();
        let func_name = GET_UNSAFE_ALLOC_FLAG_FUNC;
        //args.push(llvm::LLVMVoidTypeInContext(llcx));
        let ty = llvm::LLVMFunctionType(i8, args.as_ptr(), args.len() as c_uint, False);
        let llfn = llvm::LLVMRustGetOrInsertFunction(
            llmod,
            func_name.as_ptr().cast(),
            func_name.len(),
            ty,
        );
        if tcx.sess.default_hidden_visibility() {
            llvm::LLVMRustSetVisibility(llfn, llvm::Visibility::Hidden);
        }
        if tcx.sess.must_emit_unwind_tables() {
            let uwtable =
                attributes::uwtable_attr(llcx, tcx.sess.opts.unstable_opts.use_sync_unwind);
            attributes::apply_to_llfn(llfn, llvm::AttributePlace::Function, &[uwtable]);
        }
        let llbb = llvm::LLVMAppendBasicBlockInContext(llcx, llfn, c"entry".as_ptr().cast());

        let llbuilder = llvm::LLVMCreateBuilderInContext(llcx);
        llvm::LLVMPositionBuilderAtEnd(llbuilder, llbb);

        let name = ALLOC_IS_UNSAFE;
        let ll_g = llvm::LLVMRustGetOrInsertGlobal(llmod, name.as_ptr().cast(), name.len(), i8);
        let load_inst = llvm::LLVMRustBuildGlobalNonatomicLoad(
            &llbuilder,
            i8,
            ll_g,
            "%unsafe_flag".as_ptr().cast(),
            1, /*Volatile */
        );
        llvm::LLVMBuildRet(llbuilder, load_inst);

        llvm::LLVMDisposeBuilder(llbuilder);
    }
    //set
    unsafe {
        let i8 = llvm::LLVMInt8TypeInContext(llcx);
        let mut args = Vec::with_capacity(1);
        let func_name = SET_UNSAFE_ALLOC_FLAG_FUNC;
        args.push(i8);
        let ty = llvm::LLVMFunctionType(
            llvm::LLVMVoidTypeInContext(llcx),
            args.as_ptr(),
            args.len() as c_uint,
            False,
        );
        let llfn = llvm::LLVMRustGetOrInsertFunction(
            llmod,
            func_name.as_ptr().cast(),
            func_name.len(),
            ty,
        );
        if tcx.sess.default_hidden_visibility() {
            llvm::LLVMRustSetVisibility(llfn, llvm::Visibility::Hidden);
        }
        if tcx.sess.must_emit_unwind_tables() {
            let uwtable =
                attributes::uwtable_attr(llcx, tcx.sess.opts.unstable_opts.use_sync_unwind);
            attributes::apply_to_llfn(llfn, llvm::AttributePlace::Function, &[uwtable]);
        }
        let llbb = llvm::LLVMAppendBasicBlockInContext(llcx, llfn, c"entry".as_ptr().cast());

        let llbuilder = llvm::LLVMCreateBuilderInContext(llcx);
        llvm::LLVMPositionBuilderAtEnd(llbuilder, llbb);

        let name = ALLOC_IS_UNSAFE;
        let ll_g = llvm::LLVMRustGetOrInsertGlobal(llmod, name.as_ptr().cast(), name.len(), i8);

        llvm::LLVMRustBuildGlobalNonatomicStore(
            &llbuilder,
            llvm::LLVMGetParam(llfn, 0),
            ll_g,
            1, /*Volatile */
        );
        llvm::LLVMBuildRetVoid(llbuilder);

        llvm::LLVMDisposeBuilder(llbuilder);
    }
}
