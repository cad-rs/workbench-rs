// Rhai demo central view: same mechanism as the panel, mounted in a tab.
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

fn view(ctx) {
    column([
        label::Label(#{
            text: "Rhai 脚本视图（中央标签）",
            description: "由 register_view 注册，内容与脚本面板同为声明式 UiNode",
        }),
        label::Label(#{ text: `浏览次数：${ctx.get_state("count")}` }),
    ]).with_style(
        style()
            .gap(px(12))
            .padding(px(16))
            .background(theme_color("surface")),
    )
}
