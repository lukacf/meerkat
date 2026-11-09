// Developer-owned sidebar panel, independent of MCP Apps.
export default [{
    id: "example/results",
    title: "Results",
    validateParams: params => params !== null && typeof params === "object" && typeof params.collection === "string",
    mount(container, context) {
      const heading = document.createElement("h2");
      heading.textContent = "Application results";
      const description = document.createElement("p");
      description.textContent = "This custom panel shares the stock console's tabs and split panes.";
      const status = document.createElement("p");
      const source = document.createElement("p");
      const update = next => {
        status.textContent = next.readOnly ? "View only" : "Interactive console";
        source.textContent = `${next.panel?.params?.collection ?? "All records"} for ${next.conversation?.identity ?? "this application"}`;
      };
      update(context);
      container.append(heading, description, source, status);
      return { update, dispose() { container.replaceChildren(); } };
    },
  }];
