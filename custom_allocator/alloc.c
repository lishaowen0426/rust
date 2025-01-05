#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define BUFFER_SIZE (1024 * 1024) // 1MB
#define MAX_STACK_DEPTH 1024
__thread uint8_t buffer[BUFFER_SIZE];
__thread size_t offset_stack[MAX_STACK_DEPTH];
__thread size_t stack_top = 0; // Points to the next free slot in the stack
__thread size_t current_offset = 0;

#define ALIGN_UP(addr, align) (((addr) + (align - 1)) & ~(align - 1))

void miri_setup_unsafe_stack() {
  if (stack_top >= MAX_STACK_DEPTH) {

    fprintf(stderr, "miri_setup_unsafe_stack: stack overflow!\n");
    abort();
  }

  offset_stack[stack_top++] = current_offset;
}

void *miri_unsafe_alloc(size_t size, size_t alignment) {
  size_t aligned_offset = ALIGN_UP(current_offset, alignment);
  if (aligned_offset + size > BUFFER_SIZE) {
    fprintf(stderr,
            "unsafe_alloc: Out of Memory. Requested: %zu bytes, Available: %zu "
            "bytes\n",
            size, BUFFER_SIZE - current_offset);
    abort();
  }

  void *ptr = &buffer[aligned_offset];
  current_offset = aligned_offset + size;
  return ptr;
}
void miri_reset_unsafe_alloc() {
  if (stack_top == 0) {
    fprintf(stderr, "miri_reset_unsafe_alloc: stack underflow!\n");
    abort();
  }
  size_t end = current_offset;
  current_offset = offset_stack[--stack_top];
  memset(buffer + current_offset, 0, end - current_offset);
}