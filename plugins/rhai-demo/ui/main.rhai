// Rhai demo panel: custom UI defined entirely by this script (gpui-rhai).
// Registry components compose into a UiNode tree; state lives in the script
// (ctx.get_state/set_state) and events are named functions (Fn("name")).
import "components/button" as button;
import "components/label" as label;

fn state_schema() {
    #{
        fields: #{
            count: #{
                schema: #{ type: "integer" },
                "default": #{ type: "integer", value: 0 },
            },
        },
    }
}

fn clicked(ctx, payload) {
    ctx.set_state("count", ctx.get_state("count") + 1);
}

fn view(ctx) {
    column([
        label::Label(#{
            text: "Rhai 脚本面板",
            description: "本面板的 UI 完全由本脚本定义（声明式 UiNode + 回调函数）",
        }),
        label::Label(#{ text: `点击次数：${ctx.get_state("count")}` }),
        button::Button(#{
            text: "加一",
            variant: "primary",
            on_click: Fn("clicked"),
        }),
    ]).with_style(
        style()
            .gap(px(12))
            .padding(px(16))
            .background(theme_color("surface")),
    )
}
