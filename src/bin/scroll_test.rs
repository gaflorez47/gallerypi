// Full scroll chain test: scroll-event → scroll-tick-counter → scroller visibility.
// Mirrors the exact pattern in gallery.slint + month_scroller.slint.
// Run with: DISPLAY=localhost:0 cargo run --bin scroll_test
slint::slint! {

    // Mirrors month_scroller.slint's show/hide logic
    component FakeScroller {
        in property <int> scroll-tick: 0;
        private property <bool> show-scroller: false;

        changed scroll-tick => {
            show-scroller = true;
            hide-timer.running = false;
            hide-timer.running = true;
        }

        hide-timer := Timer {
            interval: 2000ms;
            triggered => {
                show-scroller = false;
                hide-timer.running = false;
            }
        }

        width: 60px;
        Rectangle {
            background: show-scroller ? #3a5a8a : #333;
            border-radius: 8px;
            animate background { duration: 300ms; }
            Text {
                text: show-scroller ? "VISIBLE" : "hidden";
                color: #fff;
                font-size: 11px;
                horizontal-alignment: center;
                vertical-alignment: center;
            }
        }
    }

    export component TestWindow inherits Window {
        title: "Scroll Test";
        width: 480px;
        height: 600px;
        background: #111;

        out property <int> out-scroll-event-count: scroll-event-count;
        out property <int> out-changed-formula-count: changed-formula-count;
        out property <int> out-tick-counter: scroll-tick-counter;
        out property <float> out-vp-y: flick.viewport-y / 1px;
        callback print-stats();

        private property <int> scroll-event-count: 0;
        private property <int> scroll-tick-counter: 0;

        private property <length> vp-formula: flick.viewport-y;
        private property <int> changed-formula-count: 0;
        changed vp-formula => {
            changed-formula-count = changed-formula-count + 1;
            root.scroll-tick-counter = root.scroll-tick-counter + 1;
        }

        VerticalLayout {
            Rectangle {
                height: 220px;
                background: #222;
                HorizontalLayout {
                    VerticalLayout {
                        padding: 8px;
                        spacing: 6px;
                        Text { color: #fff; font-size: 12px;
                            text: "scroll-event fires:     " + scroll-event-count; }
                        Text { color: #0f0; font-size: 12px;
                            text: "changed formula fires:  " + changed-formula-count; }
                        Text { color: #ff0; font-size: 12px;
                            text: "scroll-tick-counter:    " + scroll-tick-counter; }
                        Text { color: #fff; font-size: 12px;
                            text: "viewport-y:             " + (flick.viewport-y / 1px); }
                        Text { color: #aaa; font-size: 11px;
                            text: "Scroller at right →"; }
                        Rectangle {
                            height: 32px;
                            width: 130px;
                            background: #3a5a8a;
                            border-radius: 4px;
                            TouchArea { clicked => { print-stats(); } }
                            Text {
                                text: "Print to terminal";
                                color: #fff;
                                font-size: 12px;
                                horizontal-alignment: center;
                                vertical-alignment: center;
                            }
                        }
                    }
                    // The scroller — should turn blue when scrolling
                    FakeScroller {
                        scroll-tick: root.scroll-tick-counter;
                        height: parent.height;
                    }
                }
            }

            Rectangle {
                flick := Flickable {
                    viewport-height: 20 * 48px;
                    VerticalLayout {
                        for i in [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19]: Rectangle {
                            height: 48px;
                            background: mod(i, 2) == 0 ? #2a2a2a : #333333;
                            Text {
                                text: "Row " + i;
                                color: #ccc;
                                font-size: 14px;
                                vertical-alignment: center;
                            }
                        }
                    }
                }

                // Overlay TouchArea — same pattern as gallery.slint
                TouchArea {
                    x: 0; y: 0;
                    width: parent.width;
                    height: parent.height;

                    scroll-event(e) => {
                        scroll-event-count = scroll-event-count + 1;
                        flick.viewport-y = clamp(
                            flick.viewport-y + e.delta-y,
                            -(flick.viewport-height - flick.height),
                            0px
                        );
                        root.scroll-tick-counter = root.scroll-tick-counter + 1;
                        EventResult.accept
                    }
                }
            }
        }
    }
}

fn main() {
    let window = TestWindow::new().unwrap();
    window.on_print_stats({
        let w = window.as_weak();
        move || {
            let w = w.upgrade().unwrap();
            println!("--- scroll test stats ---");
            println!("scroll-event fires:        {}", w.get_out_scroll_event_count());
            println!("changed formula fires:     {}", w.get_out_changed_formula_count());
            println!("scroll-tick-counter:       {}", w.get_out_tick_counter());
            println!("viewport-y:                {}", w.get_out_vp_y());
        }
    });
    window.run().unwrap();
}
