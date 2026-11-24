import type { VaporComponent } from "vue";

/**
 * BOUNDARY: vize's type checker models every SFC as a VDOM component, so a Vapor-compiled SFC reaches the Vapor app as
 * `unknown` and is checked here for the marker `defineVaporComponent` sets.
 */
export function vaporComponent(component: unknown): VaporComponent {
  if (
    typeof component !== "object" ||
    component === null ||
    !("__vapor" in component) ||
    component.__vapor !== true
  ) {
    throw new TypeError(
      "The component was not compiled for Vapor; check compiler.vapor in vize.config.ts",
    );
  }
  return component as VaporComponent;
}
