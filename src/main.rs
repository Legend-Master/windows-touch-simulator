use std::{sync::Mutex, thread::sleep, time::Duration};
use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
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
    ($expression:expr) => {
        if let Err(error) = $expression {
            println!("{error}");
        }
    };
}

static mut CURRENT_TOUCH_INFOS: Mutex<Vec<POINTER_TOUCH_INFO>> = Mutex::new(Vec::new());
static mut AUTO_ZOOMING: bool = false;

fn main() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).unwrap() };
    let hmod = unsafe { GetModuleHandleW(w!("kernel32.dll")).unwrap() };
    unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), hmod, 0).unwrap() };

    unsafe { InitializeTouchInjection(2, TOUCH_FEEDBACK_DEFAULT).unwrap() }

    // keep touch contacts alive
    std::thread::spawn(|| loop {
        sleep(Duration::from_millis(100));
        let touch_infos = unsafe { CURRENT_TOUCH_INFOS.lock().unwrap() };
        if !touch_infos.is_empty() {
            unsafe { log_error!(InjectTouchInput(&touch_infos)) };
        }
    });

    let message: *mut MSG = std::ptr::null_mut();
    while unsafe { GetMessageW(message, HWND::default(), 0, 0).into() } {}
}

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code < 0 {
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
            if !AUTO_ZOOMING && is_key_down(VK_RSHIFT) {
                println!("sending touch down");
                let touch_info = make_touch_info(
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
                } else {
                    println!("{result:?}");
                }
                return LRESULT(1);
            }
        }
        WM_MOUSEMOVE => {
            let mut touch_infos = unsafe { CURRENT_TOUCH_INFOS.lock().unwrap() };
            if let Some(touch_info) = touch_infos.last_mut() {
                touch_info.pointerInfo.ptPixelLocation = info.pt;
                log_error!(InjectTouchInput(&touch_infos));
            }
        }
        WM_LBUTTONUP => {
            let mut touch_infos = unsafe { CURRENT_TOUCH_INFOS.lock().unwrap() };
            if !touch_infos.is_empty() {
                println!("sending touch up");
                for touch_info in touch_infos.iter_mut() {
                    touch_info.pointerInfo.pointerFlags = POINTER_FLAG_UP;
                }
                log_error!(InjectTouchInput(&touch_infos));
                touch_infos.clear();
                return LRESULT(1);
            }
        }
        WM_MOUSEWHEEL => {
            if !AUTO_ZOOMING && is_key_down(VK_RSHIFT) {
                let zoom_out = HIWORD(info.mouseData) < 0;
                let mut first_contact = make_touch_info(
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
                    AUTO_ZOOMING = true;
                    std::thread::spawn(move || {
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
                            if let Err(error) = InjectTouchInput(&touch_infos) {
                                println!("Error while performing auto zoom: {error}");
                                break;
                            }
                        }
                        sleep(Duration::from_millis(5));
                        for touch_info in touch_infos.iter_mut() {
                            touch_info.pointerInfo.pointerFlags = POINTER_FLAG_UP;
                        }
                        log_error!(InjectTouchInput(&touch_infos));
                        AUTO_ZOOMING = false
                    });
                    return LRESULT(1);
                } else {
                    println!("{result:?}");
                }
            }
        }
        _ => {}
    };

    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

fn make_touch_info(point: &POINT, flags: POINTER_FLAGS) -> POINTER_TOUCH_INFO {
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
    unsafe { GetAsyncKeyState(key.0.into()) & 1 != 0 }
}

#[allow(non_snake_case)]
pub fn HIWORD(l: u32) -> i16 {
    ((l >> 16) & 0xffff) as _
}
