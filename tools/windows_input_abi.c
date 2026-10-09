/* Compile-only Windows SDK oracle. Never installs hooks or injects input. */
#define WIN32_LEAN_AND_MEAN
#define WINVER 0x0a00
#define _WIN32_WINNT 0x0a00
#include <windows.h>
#include <stddef.h>

#define CHECK(expr) _Static_assert((expr), #expr)
#define WIDE (sizeof(void *) == 8)
#ifndef KEYWEAVE_EXPECT_PTR
#error Pass /DKEYWEAVE_EXPECT_PTR=4 or 8 for the intended target
#endif
CHECK(sizeof(void *) == KEYWEAVE_EXPECT_PTR);
CHECK(sizeof(void *) == 4 || sizeof(void *) == 8);
CHECK(sizeof(WORD) == 2);
CHECK(sizeof(DWORD) == 4);
CHECK(sizeof(LONG) == 4);
CHECK(sizeof(BOOL) == 4);
CHECK(sizeof(UINT) == 4);
CHECK(sizeof(LRESULT) == sizeof(void *));
CHECK(sizeof(WPARAM) == sizeof(void *));
CHECK(sizeof(LPARAM) == sizeof(void *));
CHECK(sizeof(ULONG_PTR) == sizeof(void *));
CHECK(sizeof(KEYBDINPUT) == (WIDE ? 24 : 16));
CHECK(offsetof(KEYBDINPUT, wVk) == 0);
CHECK(offsetof(KEYBDINPUT, wScan) == 2);
CHECK(offsetof(KEYBDINPUT, dwFlags) == 4);
CHECK(offsetof(KEYBDINPUT, time) == 8);
CHECK(offsetof(KEYBDINPUT, dwExtraInfo) == (WIDE ? 16 : 12));
CHECK(sizeof(MOUSEINPUT) == (WIDE ? 32 : 24));
CHECK(offsetof(MOUSEINPUT, dx) == 0);
CHECK(offsetof(MOUSEINPUT, dy) == 4);
CHECK(offsetof(MOUSEINPUT, mouseData) == 8);
CHECK(offsetof(MOUSEINPUT, dwFlags) == 12);
CHECK(offsetof(MOUSEINPUT, time) == 16);
CHECK(offsetof(MOUSEINPUT, dwExtraInfo) == (WIDE ? 24 : 20));
CHECK(sizeof(HARDWAREINPUT) == 8);
CHECK(offsetof(HARDWAREINPUT, uMsg) == 0);
CHECK(offsetof(HARDWAREINPUT, wParamL) == 4);
CHECK(offsetof(HARDWAREINPUT, wParamH) == 6);
CHECK(sizeof(INPUT) == (WIDE ? 40 : 28));
CHECK(offsetof(INPUT, type) == 0);
CHECK(offsetof(INPUT, ki) == (WIDE ? 8 : 4));
CHECK(_Alignof(INPUT) == _Alignof(ULONG_PTR));
CHECK(sizeof(KBDLLHOOKSTRUCT) == (WIDE ? 24 : 20));
CHECK(offsetof(KBDLLHOOKSTRUCT, vkCode) == 0);
CHECK(offsetof(KBDLLHOOKSTRUCT, scanCode) == 4);
CHECK(offsetof(KBDLLHOOKSTRUCT, flags) == 8);
CHECK(offsetof(KBDLLHOOKSTRUCT, time) == 12);
CHECK(offsetof(KBDLLHOOKSTRUCT, dwExtraInfo) == 16);
CHECK(sizeof(MSG) == (WIDE ? 48 : 32));
CHECK(offsetof(MSG, hwnd) == 0);
CHECK(offsetof(MSG, message) == (WIDE ? 8 : 4));
CHECK(offsetof(MSG, wParam) == (WIDE ? 16 : 8));
CHECK(offsetof(MSG, lParam) == (WIDE ? 24 : 12));
CHECK(offsetof(MSG, time) == (WIDE ? 32 : 16));
CHECK(offsetof(MSG, pt) + offsetof(POINT, x) == (WIDE ? 36 : 20));
CHECK(offsetof(MSG, pt) + offsetof(POINT, y) == (WIDE ? 40 : 24));
CHECK(offsetof(MSG, lPrivate) == (WIDE ? 44 : 28));
CHECK(INPUT_KEYBOARD == 1);
CHECK(KEYEVENTF_SCANCODE == 8);
CHECK(KEYEVENTF_EXTENDEDKEY == 1);
CHECK(KEYEVENTF_KEYUP == 2);
CHECK(KEYEVENTF_UNICODE == 4);
CHECK(LLKHF_EXTENDED == 1);
CHECK(LLKHF_LOWER_IL_INJECTED == 2);
CHECK(LLKHF_INJECTED == 0x10);
CHECK(LLKHF_ALTDOWN == 0x20);
CHECK(LLKHF_UP == 0x80);
CHECK(WM_KEYDOWN == 0x100 && WM_KEYUP == 0x101);
CHECK(WM_SYSKEYDOWN == 0x104 && WM_SYSKEYUP == 0x105);
CHECK(WM_QUIT == 0x12);
CHECK(WH_KEYBOARD_LL == 13 && HC_ACTION == 0);
CHECK(QS_ALLINPUT == 0x1cff && MWMO_INPUTAVAILABLE == 4 && PM_REMOVE == 1);
CHECK(WAIT_OBJECT_0 == 0 && WAIT_TIMEOUT == 258 && WAIT_FAILED == 0xffffffff);

/* SDK prototype checks including WINAPI calling convention (especially x86).
   Compile with /WX: incompatible pointer assignment must fail. No calls. */
int main(void) {
    UINT (WINAPI *send_input)(UINT, LPINPUT, int) = SendInput;
    HHOOK (WINAPI *install)(int, HOOKPROC, HINSTANCE, DWORD) = SetWindowsHookExW;
    BOOL (WINAPI *remove_hook)(HHOOK) = UnhookWindowsHookEx;
    LRESULT (WINAPI *next_hook)(HHOOK, int, WPARAM, LPARAM) = CallNextHookEx;
    BOOL (WINAPI *get_message)(LPMSG, HWND, UINT, UINT) = GetMessageW;
    BOOL (WINAPI *peek_message)(LPMSG, HWND, UINT, UINT, UINT) = PeekMessageW;
    DWORD (WINAPI *wait_message)(DWORD, const HANDLE *, DWORD, DWORD, DWORD) = MsgWaitForMultipleObjectsEx;
    BOOL (WINAPI *translate)(const MSG *) = TranslateMessage;
    LRESULT (WINAPI *dispatch)(const MSG *) = DispatchMessageW;
    void (WINAPI *quit)(int) = PostQuitMessage;
    HMODULE (WINAPI *module)(LPCWSTR) = GetModuleHandleW;
    DWORD (WINAPI *get_error)(void) = GetLastError;
    void (WINAPI *set_error)(DWORD) = SetLastError;
    (void)send_input; (void)install; (void)remove_hook; (void)next_hook;
    (void)get_message; (void)peek_message; (void)wait_message;
    (void)translate; (void)dispatch; (void)quit; (void)module;
    (void)get_error; (void)set_error;
    return 0;
}
