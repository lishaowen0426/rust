#include "UnsafeAllocPass.h"
#include "llvm/ADT/SmallVector.h"
#include "llvm/IR/Constants.h"
#include "llvm/IR/DataLayout.h"
#include "llvm/IR/Function.h"
#include "llvm/IR/InstIterator.h"
#include "llvm/IR/Instructions.h"
#include "llvm/IR/IntrinsicInst.h"
#include "llvm/IR/Intrinsics.h"
#include "llvm/IR/IntrinsicsX86.h"
#include "llvm/IR/LLVMContext.h"
#include "llvm/IR/MDBuilder.h"
#include "llvm/IR/Metadata.h"
#include "llvm/IR/Type.h"
#include "llvm/IR/Value.h"
#include "llvm/Support/Casting.h"
#include "llvm/Support/raw_ostream.h"
#include "llvm/Transforms/Utils/BasicBlockUtils.h"

using namespace llvm;

AnalysisKey CollectAllocaAnalysis::Key;

CollectAllocaAnalysis::Result
CollectAllocaAnalysis::run(Function &F, FunctionAnalysisManager &FAM) {
  Result result;
  for (inst_iterator I = inst_begin(F), E = inst_end(F); I != E; I++) {
    if (auto *alloca = dyn_cast<AllocaInst>(&(*I))) {
      if (alloca->hasMetadata("unsafe_rust")) {
        result.emplace_back(alloca);
      }
    }
  }

  return result;
}

AnalysisKey CollectLifetimeEndAnalysis::Key;

CollectLifetimeEndAnalysis::Result
CollectLifetimeEndAnalysis::run(Function &F, FunctionAnalysisManager &FAM) {
  Result result;
  for (inst_iterator I = inst_begin(F), E = inst_end(F); I != E; I++) {

    if (auto *II = dyn_cast<IntrinsicInst>(&(*I))) {
      if (II->getIntrinsicID() == llvm::Intrinsic::lifetime_end &&
          II->hasMetadata("unsafe_rust")) {
        result.emplace_back(II);
      }
    }
  }

  return result;
}

PreservedAnalyses ReplaceAllocaPass::run(Function &F,
                                         FunctionAnalysisManager &FAM) {
  LLVMContext &llctx = F.getContext();
  Function *malloc_func = F.getParent()->getFunction("malloc_unsafe");
  assert(malloc_func && "Cannot find malloc function");
  FunctionType *malloc_type = malloc_func->getFunctionType();
  auto int64ty = Type::getInt64Ty(llctx);

  const DataLayout &data_layout = F.getParent()->getDataLayout();

  const auto result = FAM.getResult<CollectAllocaAnalysis>(F);
  for (auto alloca : result) {
    if (alloca->hasMetadata("unsafe_rust")) {
      SmallVector<Value *, 1> args(1);
      auto alloca_size = ConstantInt::get(
          int64ty, alloca->getAllocationSize(data_layout).value());
      args[0] = alloca_size;
      CallInst *call_intrinsic =
          CallInst::Create(malloc_type, malloc_func, args);
      MDNode *md = alloca->getMetadata("unsafe_rust");
      call_intrinsic->setMetadata("unsafe_rust", md);
      ReplaceInstWithInst(alloca, call_intrinsic);
    }
  }
  auto PA = PreservedAnalyses::none();
  return PA;
}

PreservedAnalyses ReplaceLifetimeEndPass::run(Function &F,
                                              FunctionAnalysisManager &FAM) {
  LLVMContext &llctx = F.getContext();
  Function *free_func = F.getParent()->getFunction("free");
  assert(free_func && "Cannot find free function");
  FunctionType *free_type = free_func->getFunctionType();
  const auto result = FAM.getResult<CollectLifetimeEndAnalysis>(F);

  for (auto lifetimeInst : result) {
    SmallVector<Value *, 1> args(1);
    Value *ptr = lifetimeInst->getArgOperand(1);
    args[0] = ptr;

    CallInst *call_free = CallInst::Create(free_type, free_func, args);
    call_free->insertBefore(lifetimeInst);
  }
  auto PA = PreservedAnalyses::none();
  return PA;
}