.section .text

.macro EXCEPTION vector, error=0
.global arkhe_exception_\vector
arkhe_exception_\vector:
    .if !\error
        push 0
    .endif
    push \vector
    jmp arkhe_exception_common
.endm

EXCEPTION 0
EXCEPTION 1
EXCEPTION 2
EXCEPTION 3
EXCEPTION 4
EXCEPTION 5
EXCEPTION 6
EXCEPTION 7
EXCEPTION 8, 1
EXCEPTION 9
EXCEPTION 10, 1
EXCEPTION 11, 1
EXCEPTION 12, 1
EXCEPTION 13, 1
EXCEPTION 14, 1
EXCEPTION 15
EXCEPTION 16
EXCEPTION 17, 1
EXCEPTION 18
EXCEPTION 19
EXCEPTION 20
EXCEPTION 21, 1
EXCEPTION 22
EXCEPTION 23
EXCEPTION 24
EXCEPTION 25
EXCEPTION 26
EXCEPTION 27
EXCEPTION 28
EXCEPTION 29, 1
EXCEPTION 30, 1
EXCEPTION 31
EXCEPTION 128

.global arkhe_unexpected_entry
arkhe_unexpected_entry:
    push 0
    push 255

arkhe_exception_common:
    cld
    push rax
    push rcx
    push rdx
    push rbx
    push rbp
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov rdi, rsp
    mov rbx, rsp
    and rsp, -16
    call arkhe_exception_dispatch
    mov rsp, rbx
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rbx
    pop rdx
    pop rcx
    pop rax
    add rsp, 16
    iretq

.section .rodata
.global arkhe_exception_entries
.align 8
arkhe_exception_entries:
.quad arkhe_exception_0, arkhe_exception_1, arkhe_exception_2, arkhe_exception_3
.quad arkhe_exception_4, arkhe_exception_5, arkhe_exception_6, arkhe_exception_7
.quad arkhe_exception_8, arkhe_exception_9, arkhe_exception_10, arkhe_exception_11
.quad arkhe_exception_12, arkhe_exception_13, arkhe_exception_14, arkhe_exception_15
.quad arkhe_exception_16, arkhe_exception_17, arkhe_exception_18, arkhe_exception_19
.quad arkhe_exception_20, arkhe_exception_21, arkhe_exception_22, arkhe_exception_23
.quad arkhe_exception_24, arkhe_exception_25, arkhe_exception_26, arkhe_exception_27
.quad arkhe_exception_28, arkhe_exception_29, arkhe_exception_30, arkhe_exception_31
