#ifndef UNSAFE_ALLOC_PASS_H
#define UNSAFE_ALLOC_PASS_H

#include "llvm/ADT/SmallVector.h"
#include "llvm/IR/Instructions.h"
#include "llvm/IR/IntrinsicInst.h"
#include "llvm/IR/PassManager.h"

namespace llvm {

class CollectAllocaAnalysis : public AnalysisInfoMixin<CollectAllocaAnalysis> {
  friend AnalysisInfoMixin<CollectAllocaAnalysis>;
  static AnalysisKey Key;

public:
  using Result = SmallVector<AllocaInst *>;
  Result run(Function &F, FunctionAnalysisManager &FAM);
};

class CollectLifetimeEndAnalysis
    : public AnalysisInfoMixin<CollectLifetimeEndAnalysis> {
  friend AnalysisInfoMixin<CollectLifetimeEndAnalysis>;
  static AnalysisKey Key;

public:
  using Result = SmallVector<IntrinsicInst *>;
  Result run(Function &F, FunctionAnalysisManager &FAM);
};

class ReplaceAllocaPass : public PassInfoMixin<ReplaceAllocaPass> {
public:
  PreservedAnalyses run(Function &F, FunctionAnalysisManager &FAM);
};

class ReplaceLifetimeEndPass : public PassInfoMixin<ReplaceLifetimeEndPass> {
public:
  PreservedAnalyses run(Function &F, FunctionAnalysisManager &FAM);
};

} // namespace llvm

#endif
