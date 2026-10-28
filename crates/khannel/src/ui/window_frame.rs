//! 창 프레임: Linux client-side decorations(CSD)의 그림자/테두리/가장자리 리사이즈.
//!
//! 시스템 타이틀바를 쓰지 않으므로(`WindowDecorations::Client`) 창 테두리도 앱이 그린다.
//! zed `workspace::client_side_decorations`를 단순화해 옮겼다.
//! - 컴포지터가 창을 타일링 중인 변(niri 등 타일링 WM의 기본)에는 아무것도 그리지 않는다.
//! - 플로팅(타일링 아님) 변에는 그림자 영역을 두고, 그 영역을 잡아 끌면 리사이즈한다.
//! - macOS는 항상 `Decorations::Server`라 그대로 통과한다(네이티브 창 테두리 사용).

use gpui::{
    canvas, div, point, prelude::*, px, size, App, Bounds, CursorStyle, Decorations, Hitbox,
    HitboxBehavior, Hsla, IntoElement, MouseButton, Pixels, Point, ResizeEdge, Size, Styled,
    Tiling, Window,
};

use crate::theme::Theme;

/// 그림자(= 리사이즈 핸들) 두께
const SHADOW: Pixels = px(10.);
/// 창 테두리 두께
const BORDER: Pixels = px(1.);

/// `content`를 창 프레임으로 감싼다.
pub fn window_frame(content: impl IntoElement, window: &mut Window, cx: &App) -> gpui::Div {
    let decorations = window.window_decorations();
    let tiling = match decorations {
        Decorations::Server => {
            window.set_client_inset(px(0.));
            return div().size_full().child(content);
        }
        Decorations::Client { tiling } => tiling,
    };
    window.set_client_inset(SHADOW);

    let theme = Theme::current(cx);
    let is_resizable = window.is_resizable();

    div()
        .size_full()
        .when(!tiling.top, |d| d.pt(SHADOW))
        .when(!tiling.bottom, |d| d.pb(SHADOW))
        .when(!tiling.left, |d| d.pl(SHADOW))
        .when(!tiling.right, |d| d.pr(SHADOW))
        .when(is_resizable, |d| {
            d.on_mouse_down(MouseButton::Left, move |event, window, _| {
                let window_size = window.window_bounds().get_bounds().size;
                if let Some(edge) = resize_edge(event.position, window_size, tiling) {
                    window.start_window_resize(edge);
                }
            })
            // 가장자리 위에서 리사이즈 커서 표시
            .child(
                canvas(
                    |_, window, _| {
                        window.insert_hitbox(
                            Bounds::new(
                                point(px(0.), px(0.)),
                                window.window_bounds().get_bounds().size,
                            ),
                            HitboxBehavior::Normal,
                        )
                    },
                    move |_, hitbox: Hitbox, window, _| {
                        let window_size = window.window_bounds().get_bounds().size;
                        let Some(edge) = resize_edge(window.mouse_position(), window_size, tiling)
                        else {
                            return;
                        };
                        window.set_cursor_style(
                            match edge {
                                ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                                ResizeEdge::Left | ResizeEdge::Right => {
                                    CursorStyle::ResizeLeftRight
                                }
                                ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                                    CursorStyle::ResizeUpLeftDownRight
                                }
                                ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                                    CursorStyle::ResizeUpRightDownLeft
                                }
                            },
                            &hitbox,
                        );
                    },
                )
                .size_full()
                .absolute(),
            )
        })
        .child(
            div()
                .size_full()
                .cursor(CursorStyle::Arrow)
                .border_color(theme.border)
                .when(!tiling.top, |d| d.border_t(BORDER))
                .when(!tiling.bottom, |d| d.border_b(BORDER))
                .when(!tiling.left, |d| d.border_l(BORDER))
                .when(!tiling.right, |d| d.border_r(BORDER))
                .when(!tiling.is_tiled(), |d| {
                    d.shadow(vec![gpui::BoxShadow::new(
                        px(0.),
                        px(0.),
                        Hsla {
                            h: 0.,
                            s: 0.,
                            l: 0.,
                            a: 0.4,
                        },
                    )
                    .blur_radius(SHADOW / 2.)])
                })
                // 내용 위에서는 리사이즈 판정을 하지 않는다
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .child(content),
        )
}

/// 마우스 위치가 리사이즈 가장자리(그림자 영역)이면 그 변을 돌려준다. 타일링된 변은 제외.
fn resize_edge(
    pos: Point<Pixels>,
    window_size: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let bounds = Bounds::new(Point::default(), window_size).inset(SHADOW * 1.5);
    if bounds.contains(&pos) {
        return None;
    }

    let corner = size(SHADOW * 1.5, SHADOW * 1.5);
    let in_corner = |origin: Point<Pixels>| Bounds::new(origin, corner).contains(&pos);
    let right = window_size.width - corner.width;
    let bottom = window_size.height - corner.height;

    if !tiling.top && in_corner(point(px(0.), px(0.))) {
        return Some(ResizeEdge::TopLeft);
    }
    if !tiling.top && in_corner(point(right, px(0.))) {
        return Some(ResizeEdge::TopRight);
    }
    if !tiling.bottom && in_corner(point(px(0.), bottom)) {
        return Some(ResizeEdge::BottomLeft);
    }
    if !tiling.bottom && in_corner(point(right, bottom)) {
        return Some(ResizeEdge::BottomRight);
    }

    if !tiling.top && pos.y < SHADOW {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && pos.y > window_size.height - SHADOW {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && pos.x < SHADOW {
        Some(ResizeEdge::Left)
    } else if !tiling.right && pos.x > window_size.width - SHADOW {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Size<Pixels> = Size {
        width: px(800.),
        height: px(600.),
    };

    fn floating() -> Tiling {
        Tiling::default()
    }

    #[test]
    fn inside_content_is_not_an_edge() {
        assert_eq!(
            resize_edge(point(px(400.), px(300.)), WINDOW, floating()),
            None
        );
    }

    #[test]
    fn edges_and_corners_when_floating() {
        assert_eq!(
            resize_edge(point(px(400.), px(2.)), WINDOW, floating()),
            Some(ResizeEdge::Top)
        );
        assert_eq!(
            resize_edge(point(px(798.), px(300.)), WINDOW, floating()),
            Some(ResizeEdge::Right)
        );
        assert_eq!(
            resize_edge(point(px(2.), px(2.)), WINDOW, floating()),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            resize_edge(point(px(798.), px(598.)), WINDOW, floating()),
            Some(ResizeEdge::BottomRight)
        );
    }

    #[test]
    fn tiled_edges_are_not_resizable() {
        let tiled = Tiling {
            top: true,
            left: true,
            right: true,
            bottom: true,
        };
        assert_eq!(resize_edge(point(px(400.), px(2.)), WINDOW, tiled), None);
        assert_eq!(resize_edge(point(px(2.), px(2.)), WINDOW, tiled), None);
    }
}
