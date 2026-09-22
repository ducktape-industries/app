use super::*;

#[gpui_kit::test]
fn same_module_instances_receive_independent_props(cx: &mut gpui_kit::TestAppContext) {
    let first = cx.new(|_| NativeModuleView::new("independent-props-test"));
    let second = cx.new(|_| NativeModuleView::new("independent-props-test"));
    first.update(cx, |view, cx| view.set_props(b"channel-one".to_vec(), cx));
    second.update(cx, |view, cx| view.set_props(b"channel-two".to_vec(), cx));
    cx.update(|cx| {
        let first = first.read(cx);
        let second = second.read(cx);
        assert_ne!(first.instance, second.instance);
        assert!(!Arc::ptr_eq(&first.seat, &second.seat));
        assert_eq!(
            first.seat.lock().unwrap().props.as_deref(),
            Some(b"channel-one".as_slice())
        );
        assert_eq!(
            second.seat.lock().unwrap().props.as_deref(),
            Some(b"channel-two".as_slice())
        );
    });
    first.update(cx, |view, cx| view.set_props(b"channel-three".to_vec(), cx));
    cx.update(|cx| {
        assert_eq!(
            second.read(cx).seat.lock().unwrap().props.as_deref(),
            Some(b"channel-two".as_slice())
        );
    });
}
