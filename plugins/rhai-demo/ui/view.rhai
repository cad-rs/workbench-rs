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
            text: "Rhai Script View (central tab)",
            description: "Registered with register_view; like the script panel, its content is a declarative UiNode",
        }),
        label::Label(#{ text: `Views: ${ctx.get_state("count")}` }),
    ]).with_style(
        style()
            .gap(px(12))
            .padding(px(16))
            .background(theme_color("surface")),
    )
}
