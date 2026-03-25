# 原理

这就是在 Windows 设备上将指定进程提高到 SYSTEM 权限的一个典型实现方案。它是通过窃取 winlogon.exe (或 lsass.exe) 等 SYSTEM 级系统进程的 Token，并复制该 Token 后利用 CreateProcessWithTokenW API 创建一个继承了系统高级权限的新进程。

enable_privilege：启用自己进程的 SeDebugPrivilege 特权，这允许我们打开并获取其他系统级别进程(如 winlogon.exe)的句柄。
获取进程句柄与 Token：遍历查找 winlogon.exe 获取其 PID，通过 OpenProcess 与 OpenProcessToken 获取系统 Token。
DuplicateTokenEx：对获取到的 SYSTEM Token进行复制，创建出一个具有完整权限的全新 Token。
CreateProcessWithTokenW：将刚刚复制好的 SYSTEM Token 分配给目标启动进程指令（默认情况使用 cmd.exe），从而实现 SYSTEM 进程的派生。