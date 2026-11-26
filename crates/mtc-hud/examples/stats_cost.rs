//! What the HUD's per-thread CPU listing costs: times each call it makes, on
//! this process's threads (a few started here so there is something to list).

use std::time::Instant;

use windows::Win32::Foundation::{CloseHandle, HLOCAL, LocalFree};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetThreadDescription, OpenThread, THREAD_QUERY_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION};
use windows::Win32::System::WindowsProgramming::QueryThreadCycleTime;

fn main() {
    for _ in 0..40 {
        std::thread::spawn(|| std::thread::park());
    }
    let pid = unsafe { GetCurrentProcessId() };
    for round in 0..5 {
        let t = Instant::now();
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.expect("snapshot");
        let snapshot_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut more = unsafe { Thread32First(snap, &mut entry) }.is_ok();
        let (mut all, mut own) = (0, Vec::new());
        while more {
            all += 1;
            if entry.th32OwnerProcessID == pid {
                own.push(entry.th32ThreadID);
            }
            more = unsafe { Thread32Next(snap, &mut entry) }.is_ok();
        }
        let walk_ms = t.elapsed().as_secs_f64() * 1000.0;
        unsafe {
            let _ = CloseHandle(snap);
        }
        let (mut open, mut cycles, mut describe) = (0.0, 0.0, 0.0);
        for &id in &own {
            let t = Instant::now();
            let Ok(h) = (unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION | THREAD_QUERY_INFORMATION, false, id) }) else { continue };
            open += t.elapsed().as_secs_f64() * 1000.0;
            let t = Instant::now();
            let mut c = 0u64;
            let _ = unsafe { QueryThreadCycleTime(h, &mut c) };
            cycles += t.elapsed().as_secs_f64() * 1000.0;
            let t = Instant::now();
            if let Ok(p) = unsafe { GetThreadDescription(h) } {
                unsafe {
                    let _ = LocalFree(Some(HLOCAL(p.0.cast())));
                }
            }
            describe += t.elapsed().as_secs_f64() * 1000.0;
            unsafe {
                let _ = CloseHandle(h);
            }
        }
        println!(
            "Pass {round}: snapshot {snapshot_ms:.1} ms · walking {all} threads {walk_ms:.1} ms · {} of this process: OpenThread {open:.2} ms · cycles {cycles:.2} ms · thread names {describe:.2} ms",
            own.len()
        );
    }
}
