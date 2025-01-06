#include "LLVMWrapper.h"
#include "SVF-LLVM/LLVMUtil.h"
#include "SVF-LLVM/SVFIRBuilder.h"

using namespace llvm;
using namespace SVF;

extern "C" LLVMRustResult LLVMSVF(LLVMModuleRef ModuleRef) {
  Module *module = unwrap(ModuleRef);
  SVFModule *svfModule = LLVMModuleSet::buildSVFModule(*module);
  SVFIRBuilder builder(svfModule);
  SVFIR *pag = builder.build();
  pag->dump("svf-pag");
  return LLVMRustResult::Success;
}