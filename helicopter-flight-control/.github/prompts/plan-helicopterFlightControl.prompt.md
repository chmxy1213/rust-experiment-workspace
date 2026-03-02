## Plan: ESP32-C3 裸机直升机飞控开发计划

基于 Rust `no_std` 环境在 ESP32-C3 (RISC-V) 上开发直升机飞控，核心目标为实现平稳的姿态控制（非 3D 特技）。核心架构包括：使用 SPI 读取高性能 IMU (ICM42688/BMI270)，利用 ESP32-C3 的硬件串口反相功能读取乐迪 AT9S Pro 的 SBUS 信号，通过姿态解算 (Mahony/Madgwick) 和串级 PID 控制直升机的 CCPM 混控器，最后通过 PWM 输出驱动舵机和电调。

**Steps**
1. **项目初始化与工具链配置**
   - 在 Cargo.toml 中引入 `esp-hal`、`esp-backtrace` 和 `esp-println` 依赖。
   - 安装 RISC-V 编译目标 (`riscv32imc-unknown-none-elf`) 和烧录工具 (`cargo-espflash`)。
   - 在 src/main.rs 中配置 `#[no_std]` 和 `#[no_main]` 入口点。
2. **外设驱动开发 (HAL 层)**
   - **IMU 通信**：在 src/main.rs 中配置 SPI 主机模式，引入 `embedded-hal` 兼容的 ICM42688 或 BMI270 驱动，实现高频读取加速度计和陀螺仪数据。
   - **遥控器接收**：配置 UART 接收乐迪 AT9S Pro (通常配合 R9DS/R12DS 接收机) 的 SBUS 信号。利用 ESP32-C3 UART 的硬件反相特性 (`rx_inv`) 直接读取 SBUS，并解析 16 通道数据。
   - **PWM 输出**：配置 `LEDC` 或 `MCPWM` 外设，生成 50Hz/333Hz PWM 信号，用于驱动直升机的 3 个十字盘舵机、1 个尾舵机（或尾电机）以及主电机 ESC。
3. **核心算法实现**
   - **姿态解算 (AHRS)**：引入 `micromath` 或 `libm`，实现 Mahony 或 Madgwick 滤波算法，将 IMU 数据转换为四元数和欧拉角 (Roll, Pitch, Yaw)。
   - **串级 PID 控制器**：实现外环（角度控制，确保姿态稳定）和内环（角速度控制）PID 算法。直升机尾轴通常需要单独的锁尾 PID 逻辑。由于目标是姿态稳定而非 3D 特技，外环 PID 的调参将侧重于平稳恢复水平姿态，并限制最大倾斜角。
   - **CCPM 混控器 (Mixer)**：实现直升机特有的 120° 或 135° 十字盘混控算法，将 Roll, Pitch, Collective (油门/总距) 映射到 3 个舵机的行程上。
4. **主循环与状态机**
   - **高频控制循环**：使用硬件定时器中断 (Timer Interrupt) 驱动主循环（例如 1000Hz），确保传感器读取、PID 计算和 PWM 输出的严格实时性。
   - **飞行状态机**：实现加锁/解锁 (Arm/Disarm) 逻辑、飞行模式切换（以姿态自稳模式为主，可扩展定高模式）以及失控保护 (Failsafe)。

**Verification**
- **硬件在环测试**：使用 `esp-println` 打印解算后的姿态角，在上位机（如 SerialPlot）中可视化，验证 IMU 滤波准确性。
- **PWM 示波器验证**：在不接桨叶的情况下，拨动遥控器摇杆，使用示波器或逻辑分析仪观察 PWM 占空比变化是否符合 CCPM 混控预期。
- **安全测试**：关闭遥控器电源，验证飞控是否能在 0.5 秒内自动切断主电机油门并进入 Failsafe 状态。

**Decisions**
- **架构选择**：选择 `no_std` 裸机环境以保证飞控主循环的微秒级确定性，避免 RTOS 调度带来的延迟抖动。
- **接收机协议**：乐迪 AT9S Pro 搭配的接收机支持 SBUS，ESP32-C3 具有硬件串口反相，无需外部反相器即可直接读取 SBUS，极大简化了硬件设计。
- **通信接口**：IMU 强烈建议使用 SPI 而非 I2C，以满足飞控 1kHz 以上的采样率需求。
