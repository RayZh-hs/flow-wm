# Extensions

Flow is designed to be extensible. Extensions can register new data types, create new node shaders, implement custom node logic, hook into existing nodes to produce novel inputs and outputs and more. Many non-critical features of Flow are implemented as extensions, including the built-in node library.

## Flow SDK

Flow provides a complete SDK in various languages for letting applications hook into Flow. An application can extend the default Flow node it represents in two ways:

1. Using the Flow SDK to implement node logic.
2. Using a Flow extension which uses the Flow SDK to extend node logic for it.

## Flow Extension

A flow extension does the following things:

1. It registers various data types at mount.
2. It declares and implements node logic for the nodes it wants to register. This includes custom node shaders.
3. It declares and implements node logic for the nodes it wants to extend. This includes custom node shaders.
4. It removes the data types at unmount.
