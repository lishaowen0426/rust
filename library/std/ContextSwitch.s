.text
.global context_switch
context_switch:


/*
    %rdi: param
    %rsi: fp
    %rdx: next_stack
*/

push    %r12    #callee-saved
mov     %rsp,   %r12  #%r12 holds the current stack

mov     %rdx,   %rsp    #load the next stack
call    %rsi    #the first parameter is already in %rdi


mov     %r12,   %rsp #restore the current stack
pop     %r12

ret