import * as Http from "@acid-cat/core-effect/Http";
import { Context, Layer } from "effect";

export class Server extends Context.Service<
  Server,
  {
    readonly client: (target: string) => Http.Client;
    readonly listenerRegistered: (target: string, event: string) => void;
  }
>()("quadratic-web/Server") {}

export function serverLayer(
  origin: string,
  listenerRegistered: (target: string, event: string) => void,
): Layer.Layer<Server> {
  const clients = new Map<string, Http.Client>();
  const client = (target: string): Http.Client => {
    const known = clients.get(target);
    if (known !== undefined) {
      return known;
    }
    const made = Http.client(new URL(`/${target}`, origin).href);
    clients.set(target, made);
    return made;
  };
  return Layer.succeed(Server, Server.of({ client, listenerRegistered }));
}
