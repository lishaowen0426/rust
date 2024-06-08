#include "UnsafeAllocPass.h"
#include "llvm/IR/Instructions.h"
#include <iostream>

using namespace llvm;

PreservedAnalyses UnsafeAllocPass::run(Function &F,
                                       FunctionAnalysisManager &AM) {
  // std::cout << "UnsafeAlloc func name: " << F.getName().str() << std::endl;
  for (BasicBlock &BB : F) {
    for (auto I = BB.begin(), E = BB.end(); I != E; I++) {
      if (isa<CallInst>(I)) {
        MDNode *mnode = I->getMetadata("unsafe_rust");
        if (mnode != nullptr) {
          //        mnode->printTree(outs());
        } else {
          //       std::cout << "no metadata found" << std::endl;
        }
      }
    }
  }
  return PreservedAnalyses::all();
}