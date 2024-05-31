#ifndef UNSAFE_ALLOC_PASS_H
#define UNSAFE_ALLOC_PASS_H

#include "llvm/IR/PassManager.h"

namespace llvm {

class UnsafeAllocPass : public PassInfoMixin<UnsafeAllocPass> {
public:
  PreservedAnalyses run(Function &F, FunctionAnalysisManager &AM);
};

} // namespace llvm

#endif
