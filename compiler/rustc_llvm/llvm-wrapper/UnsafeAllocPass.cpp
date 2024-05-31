#include "UnsafeAllocPass.h"
#include <iostream>

using namespace llvm;

PreservedAnalyses UnsafeAllocPass::run(Function &F,
                                       FunctionAnalysisManager &AM) {
  std::cout << "MyPass in function: " << F.getName().str() << std::endl;
  return PreservedAnalyses::all();
}