import os
import json
compiler_dir = "/home/swli/rust-isolation/rust/compiler"
library_dir = "/home/swli/rust-isolation/rust/library"

crates = os.listdir(compiler_dir) + os.listdir(library_dir)


print(len(crates))
print(json.dumps(crates))