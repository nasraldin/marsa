//! Keeps Backspace atomic at extended grapheme boundaries in an AppKit text view.

use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::AnyObject,
};
use objc2_app_kit::{NSResponder, NSText, NSTextInputClient as _, NSTextView, NSView};
use objc2_foundation::{NSObject, NSRange, NSRect};
use unicode_segmentation::UnicodeSegmentation as _;

define_class!(
    // SAFETY: NSTextView is the correct AppKit superclass. AppKit owns calls
    // on the main thread and all inherited methods retain their contracts.
    #[unsafe(super(NSTextView, NSText, NSView, NSResponder, NSObject))]
    #[name = "MarsaQualificationTextView"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    pub struct GraphemeTextView;

    impl GraphemeTextView {
        // SAFETY: The selector and sender signature match NSTextView's public
        // key-binding action. The opaque sender is only forwarded to AppKit.
        #[unsafe(method(deleteBackward:))]
        fn delete_backward(&self, sender: Option<&AnyObject>) {
            self.select_previous_cluster();
            // SAFETY: The receiver is initialized on the AppKit main thread.
            // The super call receives the valid, unchanged sender.
            unsafe { let _: () = msg_send![super(self), deleteBackward: sender]; }
        }
    }
);

impl GraphemeTextView {
    pub fn new(main: MainThreadMarker, rect: NSRect) -> Retained<Self> {
        let allocated = main.alloc::<Self>().set_ivars(());
        // SAFETY: The receiver has initialized unit ivars and AppKit's
        // designated NSTextView initializer receives a valid frame.
        unsafe { msg_send![super(allocated), initWithFrame: rect] }
    }

    fn select_previous_cluster(&self) {
        let selection = self.selectedRange();
        if self.hasMarkedText() || selection.length != 0 {
            return;
        }

        let text = self.string().to_string();
        let mut end = 0;
        for cluster in text.graphemes(true) {
            let start = end;
            end += cluster.encode_utf16().count();
            if end == selection.location && end - start > 1 {
                self.setSelectedRange(NSRange::new(start, end - start));
                return;
            }
            if end >= selection.location {
                return;
            }
        }
    }
}
