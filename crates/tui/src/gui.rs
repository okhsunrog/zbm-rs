use anyhow::Result;
use slint::ComponentHandle;

slint::slint! {
    component SourceRow inherits Rectangle {
        in property <int> source-index;
        in property <int> selected-source;
        in property <bool> keyboard-focus;
        in property <string> title;
        callback activated(int);

        height: 44px;
        border-radius: 8px;
        background: root.source-index == root.selected-source
            ? #223550
            : source-touch.has-hover ? #19283a : transparent;
        border-width: root.keyboard-focus && root.source-index == root.selected-source ? 1px : 0px;
        border-color: #64a8ff;

        Text {
            text: "  " + root.title;
            color: root.source-index == root.selected-source ? #deebff : #9babc0;
            font-size: 15px;
            vertical-alignment: center;
        }

        source-touch := TouchArea {
            mouse-cursor: pointer;
            clicked => { root.activated(root.source-index); }
        }
    }

    component BootRow inherits Rectangle {
        in property <int> row-index;
        in property <int> selected-index;
        in property <string> title;
        in property <string> subtitle;
        in property <string> badge;
        in property <bool> trusted: true;
        in property <bool> keyboard-focus;
        callback activated(int);

        height: 72px;
        border-radius: 10px;
        background: root.row-index == root.selected-index
            ? #243b55
            : row-touch.has-hover ? #1d2a3a : transparent;
        border-width: root.keyboard-focus && root.row-index == root.selected-index ? 1px : 0px;
        border-color: #64a8ff;

        HorizontalLayout {
            padding-left: 18px;
            padding-right: 14px;
            spacing: 14px;
            alignment: center;

            Rectangle {
                width: 8px;
                height: 8px;
                border-radius: 4px;
                background: root.trusted ? #68d391 : #f6ad55;
                horizontal-stretch: 0;
            }

            VerticalLayout {
                spacing: 4px;
                Text {
                    text: root.title;
                    color: #edf4ff;
                    font-size: 17px;
                    font-weight: 600;
                }
                Text {
                    text: root.subtitle;
                    color: #93a4b8;
                    font-size: 13px;
                }
            }

            Rectangle { horizontal-stretch: 1; }
            Rectangle {
                height: 28px;
                width: badge-text.preferred-width + 22px;
                border-radius: 14px;
                background: root.trusted ? #183d31 : #49341d;
                badge-text := Text {
                    text: root.badge;
                    color: root.trusted ? rgb(142, 230, 179) : #ffd08a;
                    font-size: 12px;
                    horizontal-alignment: center;
                    vertical-alignment: center;
                }
            }
        }

        row-touch := TouchArea {
            mouse-cursor: pointer;
            clicked => { root.activated(root.row-index); }
        }
    }

    export component BootManager inherits Window {
        in-out property <int> selected-index: 0;
        in-out property <int> selected-source: 0;
        private property <int> focus-column: 0;
        private property <string> source-title:
            root.selected-source == 0 ? "All boot environments" :
            root.selected-source == 1 ? "NixOS generations" :
            root.selected-source == 2 ? "ZFS snapshots" : "Linux kernels";
        private property <string> selected-title:
            root.selected-index == 0 ? "NixOS generation 42" :
            root.selected-index == 1 ? "NixOS generation 41" :
            root.selected-index == 2 ? "@before-kernel-update" :
            root.selected-index == 3 ? "@known-good" :
            root.selected-index == 4 ? "Linux 6.17.1-zfs" : "Recovery tools";
        private property <string> selected-root:
            root.selected-index == 2 ? "zpool/nixos@before-kernel-update" :
            root.selected-index == 3 ? "zpool/nixos@known-good" : "zpool/nixos";

        title: "zbm-rs";
        background: #0d141e;

        keyboard := FocusScope {
            width: 100%;
            height: 100%;
            init => { keyboard.focus(); }
            key-pressed(event) => {
                if event.text == Key.Tab || event.text == Key.RightArrow || event.text == "l" {
                    root.focus-column = 1;
                    return accept;
                }
                if event.text == Key.Backtab || event.text == Key.LeftArrow || event.text == "h" {
                    root.focus-column = 0;
                    return accept;
                }
                if event.text == Key.DownArrow || event.text == "j" {
                    if root.focus-column == 0 {
                        root.selected-source = min(3, root.selected-source + 1);
                        root.selected-index = root.selected-source == 0 ? 0 : root.selected-source == 1 ? 0 : root.selected-source == 2 ? 2 : 4;
                    } else {
                        root.selected-index = root.selected-source == 1
                            ? min(1, root.selected-index + 1)
                            : root.selected-source == 2
                                ? min(3, root.selected-index + 1)
                                : root.selected-source == 3
                                    ? 4
                                    : min(5, root.selected-index + 1);
                    }
                    return accept;
                }
                if event.text == Key.UpArrow || event.text == "k" {
                    if root.focus-column == 0 {
                        root.selected-source = max(0, root.selected-source - 1);
                        root.selected-index = root.selected-source == 0 ? 0 : root.selected-source == 1 ? 0 : root.selected-source == 2 ? 2 : 4;
                    } else {
                        root.selected-index = root.selected-source == 1
                            ? max(0, root.selected-index - 1)
                            : root.selected-source == 2
                                ? max(2, root.selected-index - 1)
                                : root.selected-source == 3
                                    ? 4
                                    : max(0, root.selected-index - 1);
                    }
                    return accept;
                }
                return reject;
            }

            VerticalLayout {
            padding: 26px;
            spacing: 22px;

            HorizontalLayout {
                height: 62px;
                vertical-stretch: 0;
                alignment: center;

                VerticalLayout {
                    spacing: 2px;
                    Text {
                        text: "zbm-rs";
                        color: #f4f8ff;
                        font-size: 27px;
                        font-weight: 700;
                    }
                    Text {
                        text: "Verified ZFS boot manager";
                        color: #8292a8;
                        font-size: 13px;
                    }
                }
                Rectangle { horizontal-stretch: 1; }
                Rectangle {
                    width: secure-label.preferred-width + 34px;
                    height: 34px;
                    border-radius: 17px;
                    background: #153c2f;
                    secure-label := Text {
                        text: "SECURE BOOT VERIFIED";
                        color: #8be6b0;
                        font-size: 12px;
                        font-weight: 600;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }
                }
            }

            HorizontalLayout {
                spacing: 22px;

                Rectangle {
                    width: 210px;
                    horizontal-stretch: 0;
                    border-radius: 12px;
                    background: #121c28;
                    VerticalLayout {
                        padding: 18px;
                        spacing: 10px;
                        Text { text: "BOOT SOURCES"; color: #677991; font-size: 11px; font-weight: 600; }
                        SourceRow { source-index: 0; selected-source: root.selected-source; keyboard-focus: root.focus-column == 0; title: "All entries"; activated(i) => { root.selected-source = i; root.selected-index = 0; root.focus-column = 0; } }
                        SourceRow { source-index: 1; selected-source: root.selected-source; keyboard-focus: root.focus-column == 0; title: "NixOS generations"; activated(i) => { root.selected-source = i; root.selected-index = 0; root.focus-column = 0; } }
                        SourceRow { source-index: 2; selected-source: root.selected-source; keyboard-focus: root.focus-column == 0; title: "Snapshots"; activated(i) => { root.selected-source = i; root.selected-index = 2; root.focus-column = 0; } }
                        SourceRow { source-index: 3; selected-source: root.selected-source; keyboard-focus: root.focus-column == 0; title: "Linux kernels"; activated(i) => { root.selected-source = i; root.selected-index = 4; root.focus-column = 0; } }
                        Rectangle { vertical-stretch: 1; }
                        Text { text: "POOL"; color: #677991; font-size: 11px; font-weight: 600; }
                        Text { text: "novafs  ·  ONLINE"; color: #80d9a3; font-size: 14px; }
                    }
                }

                Rectangle {
                    background: transparent;
                    VerticalLayout {
                        spacing: 8px;
                        Text { text: root.source-title; color: #f1f6ff; font-size: 22px; font-weight: 600; height: 38px; }
                        BootRow { visible: root.selected-source == 0 || root.selected-source == 1; row-index: 0; selected-index: root.selected-index; keyboard-focus: root.focus-column == 1; title: "NixOS generation 42"; subtitle: "2026-10-12  ·  current system"; badge: "CURRENT"; activated(i) => { root.selected-index = i; root.focus-column = 1; } }
                        BootRow { visible: root.selected-source == 0 || root.selected-source == 1; row-index: 1; selected-index: root.selected-index; keyboard-focus: root.focus-column == 1; title: "NixOS generation 41"; subtitle: "2026-10-10  ·  previous system"; badge: "SIGNED"; activated(i) => { root.selected-index = i; root.focus-column = 1; } }
                        BootRow { visible: root.selected-source == 0 || root.selected-source == 2; row-index: 2; selected-index: root.selected-index; keyboard-focus: root.focus-column == 1; title: "@before-kernel-update"; subtitle: "Snapshot  ·  writable clone on boot"; badge: "TRUSTED"; activated(i) => { root.selected-index = i; root.focus-column = 1; } }
                        BootRow { visible: root.selected-source == 0 || root.selected-source == 2; row-index: 3; selected-index: root.selected-index; keyboard-focus: root.focus-column == 1; title: "@known-good"; subtitle: "Snapshot  ·  3 days ago"; badge: "TRUSTED"; activated(i) => { root.selected-index = i; root.focus-column = 1; } }
                        BootRow { visible: root.selected-source == 0 || root.selected-source == 3; row-index: 4; selected-index: root.selected-index; keyboard-focus: root.focus-column == 1; title: "Linux 6.17.1-zfs"; subtitle: "Generic Linux boot target"; badge: "SIGNED"; activated(i) => { root.selected-index = i; root.focus-column = 1; } }
                        BootRow { visible: root.selected-source == 0; row-index: 5; selected-index: root.selected-index; keyboard-focus: root.focus-column == 1; title: "Recovery tools"; subtitle: "Diagnostics and reboot controls"; badge: "LOCAL"; trusted: false; activated(i) => { root.selected-index = i; root.focus-column = 1; } }
                    }
                }

                Rectangle {
                    width: 290px;
                    horizontal-stretch: 0;
                    border-radius: 12px;
                    background: #121c28;
                    VerticalLayout {
                        padding: 20px;
                        spacing: 14px;
                        Text { text: "BOOT DETAILS"; color: #677991; font-size: 11px; font-weight: 600; }
                        Text { text: root.selected-title; color: #edf4ff; font-size: 19px; font-weight: 600; wrap: word-wrap; }
                        Rectangle { height: 1px; background: #273446; }
                        Text { text: "Root dataset"; color: #72849b; font-size: 12px; }
                        Text { text: root.selected-root; color: #c7d5e8; font-size: 13px; wrap: word-wrap; }
                        Text { text: "Verification"; color: #72849b; font-size: 12px; }
                        Text { text: root.selected-index == 5 ? "Local recovery entry" : "Kernel, initramfs and arguments authorized"; color: root.selected-index == 5 ? #ffc979 : #80d9a3; font-size: 13px; wrap: word-wrap; }
                        Text { text: "TPM measurement"; color: #72849b; font-size: 12px; }
                        Text { text: root.selected-index == 5 ? "Not measured" : "Prepared for PCR 15"; color: #c7d5e8; font-size: 13px; }
                        Rectangle { vertical-stretch: 1; }
                        Rectangle {
                            height: 46px;
                            border-radius: 9px;
                            background: boot-touch.pressed ? #4389df : boot-touch.has-hover ? #68adff : #579bf0;
                            Text { text: root.selected-index == 5 ? "Open recovery" : "Boot selected entry"; color: #07111d; font-size: 14px; font-weight: 700; horizontal-alignment: center; vertical-alignment: center; }
                            boot-touch := TouchArea { mouse-cursor: pointer; }
                        }
                    }
                }
            }

            HorizontalLayout {
                height: 26px;
                vertical-stretch: 0;
                Text { text: "← → / h l / Tab  Switch panel    ↑ ↓ / j k  Navigate"; color: #6f8097; font-size: 12px; }
                Rectangle { horizontal-stretch: 1; }
                Text { text: "GUI prototype · software renderer"; color: #596b82; font-size: 12px; }
            }
            }
        }
    }
}

pub fn run() -> Result<()> {
    slint::BackendSelector::new().select()?;
    BootManager::new()?.run()?;
    Ok(())
}
