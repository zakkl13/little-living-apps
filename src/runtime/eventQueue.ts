// The single durable queue that feeds the serialized manager loop (DESIGN §3). Two producers —
// owner messages (from the poller) and worker-completion events (from the orchestrator). One
// consumer drains it, one turn at a time. Serializing turns is the core invariant that keeps
// memory + transcript coherent without locks.
//
// The queue is in-memory; persistence (queue.json) is layered on by snapshot.ts (DESIGN §11) via
// snapshot()/load(). An onEnqueue listener lets the loop wake reactively.

let seq = 0;
function nextId(): string {
  seq += 1;
  return `evt_${Date.now().toString(36)}_${seq}`;
}

export interface OwnerMessageEvent {
  kind: "owner_message";
  id: string;
  chatId: number;
  text: string;
  /** Local path to an owner-sent image (view_image is on); opens the turn as a local_image input. */
  imagePath?: string;
}
export interface WorkerEvent {
  kind: "worker_event";
  id: string;
  workerId: string;
  status: "completed" | "failed";
  summary: string;
}
export type ManagerEvent = OwnerMessageEvent | WorkerEvent;

export type NewEvent = Omit<OwnerMessageEvent, "id"> | Omit<WorkerEvent, "id">;

export interface EventQueue {
  enqueue(event: NewEvent): ManagerEvent;
  dequeue(): ManagerEvent | undefined;
  size(): number;
  isEmpty(): boolean;
  snapshot(): ManagerEvent[];
  load(events: ManagerEvent[]): void;
  onEnqueue(cb: (event: ManagerEvent) => void): void;
}

export function createEventQueue(): EventQueue {
  const items: ManagerEvent[] = [];
  const listeners: Array<(event: ManagerEvent) => void> = [];

  return {
    enqueue(event) {
      const full = { ...event, id: nextId() } as ManagerEvent;
      items.push(full);
      for (const cb of listeners) cb(full);
      return full;
    },
    dequeue() {
      return items.shift();
    },
    size() {
      return items.length;
    },
    isEmpty() {
      return items.length === 0;
    },
    snapshot() {
      return items.map((e) => ({ ...e }));
    },
    load(events) {
      items.length = 0;
      items.push(...events);
    },
    onEnqueue(cb) {
      listeners.push(cb);
    },
  };
}
