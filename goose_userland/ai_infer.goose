// FipSOs v5.5 - /bin/fips_ai (Goose Zero-GC + AVX2/SSE2 FNN2, GRU --seq-infer & --stream-audit)
// Soporta telemetria, --guard, --model FNN2, --audio-listen, --stream-audit y --seq-infer.

fn guard_label(cls: i64) -> str {
    if cls == 0 { return "SAFE"; }
    if cls == 1 { return "SUSPICIOUS"; }
    return "INTRUSION";
}

fn run_guard_mode(rate: i64, trav: i64, err403: i64, socks: i64, out_wr: i64, slots: i64) -> i64 {
    let cls: i64 = fnn_guard_infer(rate, trav, err403, socks, out_wr, slots);
    let us: i64 = fnn_last_us();
    print_guard(cls, us);
    if cls == 2 {
        log_security_alert(rate, trav, err403);
        apply_sandbox(5);
    }
    return cls;
}

fn main(argc: i64) -> i64 {
    if argc >= 2 {
        return run_guard_mode(10, 0, 0, 2, 0, 20);
    }
    let from_file: i64 = fnn_load_model("/mnt/fat0/model.fnn");
    let cpu: i64 = hwmon_read_metric("cpu", 18);
    let mem: i64 = hwmon_read_metric("mem", 32);
    let procs: i64 = sysinfo_procs();
    let io_kb: i64 = hwmon_read_metric("io", 64);

    print("[fips_ai] Goose Zero-GC + AVX2/SSE2 MLP (FipSOs v5.5)\n");
    if from_file == 1 {
        print("weights=/mnt/fat0/model.fnn\n");
    } else {
        print("weights=builtin_default\n");
    }
    fnn_set_quantized(0);
    let cls_fp32: i64 = fnn_run_telemetry(cpu, mem, procs, io_kb);
    print_infer("fp32_simd", cls_fp32, fnn_last_us());
    return 0;
}
