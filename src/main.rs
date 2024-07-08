#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{sync::Mutex, thread::sleep, time::Duration};
use windows::{
    core::{w, Owned},
    Win32::{
        Foundation::{
            GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, POINT, WAIT_TIMEOUT,
            WPARAM,
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject, INFINITE},
        },
        UI::{
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::{
                KeyboardAndMouse::{GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_RSHIFT},
                Pointer::{
                    InitializeTouchInjection, InjectTouchInput, POINTER_FLAGS, POINTER_FLAG_DOWN,
                    POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_FLAG_UP,
                    POINTER_FLAG_UPDATE, POINTER_INFO, POINTER_TOUCH_INFO, TOUCH_FEEDBACK_DEFAULT,
                },
            },
            WindowsAndMessaging::{
                CallNextHookEx, GetMessageW, SetWindowsHookExW, HHOOK, LLMHF_INJECTED, MSG,
                MSLLHOOKSTRUCT, PT_TOUCH, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
                WM_MOUSEWHEEL,
            },
        },
    },
};

macro_rules! log_error {
    ($expression: expr) => {
        if let Err(error) = $expression {
            println!("{error}");
        }
    };
    ($expression: expr, $context: literal) => {
        if let Err(error) = $expression {
            println!("Error while {}: {}", $context, error);
        }
    };
}

static mut CURRENT_TOUCH_INFOS: Mutex<Vec<POINTER_TOUCH_INFO>> = Mutex::new(Vec::new());
static mut KEEP_ALIVE_EVENT: Option<Owned<HANDLE>> = None;
static mut AUTO_ZOOMING_EVENT: Option<Owned<HANDLE>> = None;
static mut AUTO_ZOOMING: Option<bool> = None;

fn main() {
    let _mutex_handle = unsafe {
        Owned::new(CreateMutexW(None, true, w!(r"Global\WindowsTouchSimulator")).unwrap())
    };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        panic!("An instance of Windows Touch Simulator is already running");
    }

    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).unwrap() };
    let hmod = unsafe { GetModuleHandleW(w!("kernel32.dll")).unwrap() };
    unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), hmod, 0).unwrap() };

    unsafe { InitializeTouchInjection(2, TOUCH_FEEDBACK_DEFAULT).unwrap() }

    // Keep touch contacts alive
    unsafe {
        KEEP_ALIVE_EVENT.replace(Owned::new(CreateEventW(None, false, false, None).unwrap()))
    };
    std::thread::spawn(move || loop {
        unsafe { WaitForSingleObject(*KEEP_ALIVE_EVENT.as_deref().unwrap(), INFINITE) };
        while unsafe { WaitForSingleObject(*KEEP_ALIVE_EVENT.as_deref().unwrap(), 100) }
            == WAIT_TIMEOUT
        {
            let touch_infos = unsafe { CURRENT_TOUCH_INFOS.lock().unwrap() };
            if touch_infos.is_empty() {
                break;
            }
            unsafe {
                log_error!(
                    InjectTouchInput(&touch_infos),
                    "sending touch move to keep the touch alive"
                )
            };
        }
    });

    // Perform auto zooming on event
    unsafe {
        AUTO_ZOOMING_EVENT.replace(Owned::new(CreateEventW(None, false, false, None).unwrap()))
    };
    std::thread::spawn(move || loop {
        unsafe { WaitForSingleObject(*AUTO_ZOOMING_EVENT.as_deref().unwrap(), INFINITE) };
        let Some(zoom_out) = (unsafe { AUTO_ZOOMING }) else {
            continue;
        };
        let mut touch_infos = unsafe { CURRENT_TOUCH_INFOS.lock().unwrap() };
        if touch_infos.is_empty() {
            continue;
        }
        for _ in 0..25 {
            sleep(Duration::from_millis(1));
            let first_contact = touch_infos.first_mut().unwrap();
            if zoom_out {
                first_contact.pointerInfo.ptPixelLocation.x += 3;
                first_contact.pointerInfo.ptPixelLocation.y += 3;
            } else {
                first_contact.pointerInfo.ptPixelLocation.x -= 3;
                first_contact.pointerInfo.ptPixelLocation.y -= 3;
            }
            let second_contact = touch_infos.last_mut().unwrap();
            if zoom_out {
                second_contact.pointerInfo.ptPixelLocation.x -= 3;
                second_contact.pointerInfo.ptPixelLocation.y -= 3;
            } else {
                second_contact.pointerInfo.ptPixelLocation.x += 3;
                second_contact.pointerInfo.ptPixelLocation.y += 3;
            }
            if let Err(error) = unsafe { InjectTouchInput(&touch_infos) } {
                println!("Error while performing auto zoom: {error}");
                break;
            }
        }
        sleep(Duration::from_millis(5));
        for touch_info in touch_infos.iter_mut() {
            touch_info.pointerInfo.pointerFlags = POINTER_FLAG_UP;
        }
        unsafe {
            log_error!(
                InjectTouchInput(&touch_infos),
                "sending touch end in auto zooming"
            )
        };
        touch_infos.clear();
        unsafe { AUTO_ZOOMING.take() };
    });

    let message: *mut MSG = std::ptr::null_mut();
    while unsafe { GetMessageW(message, HWND::default(), 0, 0).into() } {}
}

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code < 0 || AUTO_ZOOMING.is_some() {
        return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
    }

    let info = *(lparam.0 as *const MSLLHOOKSTRUCT);
    // dbg!(&wparam);
    // dbg!(&info);
    if info.flags & LLMHF_INJECTED != 0 {
        println!("injected, info.flags: {}", info.flags);
        return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
    }
    match wparam.0 as u32 {
        WM_LBUTTONDOWN => {
            if is_key_down(VK_RSHIFT) {
                println!("sending touch down");
                let touch_info = create_touch_info(
                    &info.pt,
                    POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
                );
                let mut touch_infos = vec![touch_info];
                if is_key_down(VK_CONTROL) {
                    let mut second_contact = touch_info.clone();
                    second_contact.pointerInfo.pointerId = 1;
                    touch_infos.push(second_contact);
                };
                let result = InjectTouchInput(&touch_infos);
                if result.is_ok() {
                    for touch_info in touch_infos.iter_mut() {
                        touch_info.pointerInfo.pointerFlags =
                            POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT;
                    }
                    *CURRENT_TOUCH_INFOS.lock().unwrap() = touch_infos;
                    SetEvent(*KEEP_ALIVE_EVENT.as_deref().unwrap()).unwrap();
                } else {
                    println!("Error while sending touch down: {result:?}");
                }
                return LRESULT(1);
            }
        }
        WM_MOUSEMOVE => {
            let mut touch_infos = unsafe { CURRENT_TOUCH_INFOS.lock().unwrap() };
            if let Some(touch_info) = touch_infos.last_mut() {
                touch_info.pointerInfo.ptPixelLocation = info.pt;
                log_error!(InjectTouchInput(&touch_infos), "sending touch move");
            }
        }
        WM_LBUTTONUP => {
            let mut touch_infos = unsafe { CURRENT_TOUCH_INFOS.lock().unwrap() };
            if !touch_infos.is_empty() {
                println!("sending touch up");
                for touch_info in touch_infos.iter_mut() {
                    touch_info.pointerInfo.pointerFlags = POINTER_FLAG_UP;
                }
                log_error!(InjectTouchInput(&touch_infos), "sending touch end");
                touch_infos.clear();
                SetEvent(*KEEP_ALIVE_EVENT.as_deref().unwrap()).unwrap();
                return LRESULT(1);
            }
        }
        WM_MOUSEWHEEL => {
            if is_key_down(VK_RSHIFT) && { CURRENT_TOUCH_INFOS.lock().unwrap().is_empty() } {
                let zoom_out = HIWORD(info.mouseData) < 0;
                let mut first_contact = create_touch_info(
                    &info.pt,
                    POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
                );
                let mut second_contact = first_contact.clone();
                second_contact.pointerInfo.pointerId = 1;
                if zoom_out {
                    first_contact.pointerInfo.ptPixelLocation.x -= 100;
                    first_contact.pointerInfo.ptPixelLocation.y -= 100;
                    second_contact.pointerInfo.ptPixelLocation.x += 100;
                    second_contact.pointerInfo.ptPixelLocation.y += 100;
                }
                let mut touch_infos = vec![first_contact, second_contact];
                let result = InjectTouchInput(&touch_infos);
                if result.is_ok() {
                    for touch_info in touch_infos.iter_mut() {
                        touch_info.pointerInfo.pointerFlags =
                            POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT;
                    }
                    *CURRENT_TOUCH_INFOS.lock().unwrap() = touch_infos;
                    AUTO_ZOOMING.replace(zoom_out);
                    SetEvent(*AUTO_ZOOMING_EVENT.as_deref().unwrap()).unwrap();
                    return LRESULT(1);
                } else {
                    println!("Error while sending touch down to start auto zooming: {result:?}");
                }
            }
        }
        _ => {}
    };

    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

fn create_touch_info(point: &POINT, flags: POINTER_FLAGS) -> POINTER_TOUCH_INFO {
    POINTER_TOUCH_INFO {
        pointerInfo: POINTER_INFO {
            pointerType: PT_TOUCH,
            ptPixelLocation: *point,
            pointerFlags: flags,
            // pointerId: 1,
            // dwTime: info.time,
            ..Default::default()
        },
        // rcContact: RECT {
        //     left: point.x,
        //     right: point.x,
        //     top: point.y,
        //     bottom: point.y,
        // },
        // touchMask: TOUCH_MASK_CONTACTAREA,
        ..Default::default()
    }
}

fn is_key_down(key: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(key.0.into()) & 0x8000u16 as i16 != 0 }
}

#[allow(non_snake_case)]
pub fn HIWORD(l: u32) -> i16 {
    ((l >> 16) & 0xffff) as _
}
