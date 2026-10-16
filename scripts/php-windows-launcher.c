#define UNICODE
#define _UNICODE
#include <windows.h>
#include <stdio.h>
#include <wchar.h>

int wmain(void) {
    wchar_t app[MAX_PATH], runtime[MAX_PATH], ini[MAX_PATH];
    wchar_t cwd[MAX_PATH], main_php[MAX_PATH], command[4 * MAX_PATH];
    DWORD size = GetModuleFileNameW(NULL, app, MAX_PATH);
    if (!size || size >= MAX_PATH) return 2;
    wchar_t *slash = wcsrchr(app, L'\\');
    if (!slash) return 2;
    slash[1] = L'\0';
    swprintf(runtime, MAX_PATH, L"%lsphp-runtime.exe", app);
    swprintf(ini, MAX_PATH, L"%lsphp.ini", app);
    if (!GetCurrentDirectoryW(MAX_PATH, cwd)) return 2;
    swprintf(main_php, MAX_PATH, L"%ls\\main.php", cwd);
    if (GetFileAttributesW(main_php) == INVALID_FILE_ATTRIBUTES) {
        fwprintf(stderr, L"main.php not found in current directory: %ls\n", cwd);
        fwprintf(stderr, L"Put main.php beside sweet_surprise.exe or launch it from that folder.\n");
        return 2;
    }
    wchar_t library[MAX_PATH], fonts[MAX_PATH], path[MAX_PATH * 2];
    swprintf(library, MAX_PATH, L"%lssweet.dll", app);
    swprintf(fonts, MAX_PATH, L"%lsfonts", app);
    GetEnvironmentVariableW(L"PATH", path, MAX_PATH * 2);
    wchar_t full_path[MAX_PATH * 3];
    swprintf(full_path, MAX_PATH * 3, L"%ls;%ls", app, path);
    SetEnvironmentVariableW(L"SWEET_PHP_LIBRARY", library);
    SetEnvironmentVariableW(L"SWEET_FONT_DIR", fonts);
    SetEnvironmentVariableW(L"SWEET_APP_DIR", app);
    SetEnvironmentVariableW(L"PHP_INI_SCAN_DIR", L"");
    SetEnvironmentVariableW(L"PATH", full_path);

    swprintf(command, 4 * MAX_PATH, L"\"%ls\" -c \"%ls\" \"%ls\"", runtime, ini, main_php);
    STARTUPINFOW startup = { .cb = sizeof(startup) };
    PROCESS_INFORMATION process = {0};
    if (!CreateProcessW(runtime, command, NULL, NULL, FALSE, 0, NULL, NULL, &startup, &process)) {
        fwprintf(stderr, L"Could not start bundled PHP runtime (error %lu).\n", GetLastError());
        return 3;
    }
    WaitForSingleObject(process.hProcess, INFINITE);
    DWORD result = 1;
    GetExitCodeProcess(process.hProcess, &result);
    CloseHandle(process.hThread);
    CloseHandle(process.hProcess);
    return (int)result;
}
