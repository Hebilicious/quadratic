import * as Http from "@acid-cat/core-effect/Http";
import { Context, Layer } from "effect";
import { FetchHttpClient } from "effect/http";

export class Server extends Context.Service<
  Server,
  {
    readonly client: (target: string) => Http.Client;
    readonly listenerRegistered: (target: string, event: string) => void;
  }
>()("quadratique-web/Server") {}

/** The bearer that pairing gave the page, read at each request, and what the page does when a server refuses it. */
export interface Access {
  readonly token: () => string;
  readonly unauthorized: () => void;
}

/**
 * One client per target, each sending the bearer on every request, the event stream included. The bearer is read at
 * each request, so a new one reaches the running app's later requests without new clients. A `401` from any of them
 * calls `access.unauthorized`. A page that loses its pairing drops this layer, and with it every client.
 */
export function serverLayer(
  origin: string,
  access: Access,
  listenerRegistered: (target: string, event: string) => void,
): Layer.Layer<Server> {
  const watched: typeof globalThis.fetch = async (input, init) => {
    const headers = new Headers(input instanceof Request ? input.headers : undefined);
    new Headers(init?.headers).forEach((value, name) => {
      headers.set(name, value);
    });
    headers.set("Authorization", `Bearer ${access.token()}`);
    const answer = await globalThis.fetch(input, { ...init, headers });
    if (answer.status === 401) {
      access.unauthorized();
    }
    return answer;
  };
  const layer = FetchHttpClient.layer.pipe(
    Layer.provide(Layer.succeed(FetchHttpClient.Fetch, watched)),
  );
  const clients = new Map<string, Http.Client>();
  const client = (target: string): Http.Client => {
    const known = clients.get(target);
    if (known !== undefined) {
      return known;
    }
    const made = Http.client(new URL(`/${target}`, origin).href, { layer });
    clients.set(target, made);
    return made;
  };
  return Layer.succeed(Server, Server.of({ client, listenerRegistered }));
}
