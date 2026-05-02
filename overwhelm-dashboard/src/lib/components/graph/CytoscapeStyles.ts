import type { Stylesheet } from 'cytoscape';

// Base styling for cytoscape that can be used across all graph views.
// These are functions so they can be dynamically updated if needed, or simply static constants.

export const getBaseNodeStyles = (): Stylesheet[] => [
    {
        selector: 'node[visibilityState != "hidden"]',
        style: {
            'shape': 'ellipse',
            'width': 'data(nodeSize)',
            'height': 'data(nodeSize)',
            'background-color': 'data(fillColor)',
            'background-opacity': 0.85,
            'border-width': 'data(borderWidth)',
            'border-color': 'data(borderColor)',
            'opacity': 'data(nodeOpacity)',
            'label': '',
            'text-opacity': 0,
        } as any,
    },
    {
        selector: 'node[displayLabel]',
        style: {
            'label': 'data(displayLabel)',
            'text-opacity': 1,
            'color': '#cbd5e1',
            'font-size': 9,
            'text-outline-color': '#0b0f17',
            'text-outline-width': 2,
            'text-valign': 'center',
            'text-halign': 'right',
            'text-margin-x': 6,
            'text-max-width': '180px',
            'text-wrap': 'wrap',
            'min-zoomed-font-size': 8,
        } as any,
    },
    // Backbones — epic/project/goal on route. Squared, muted, small label.
    {
        selector: 'node[isBackbone = 1]',
        style: {
            'shape': 'round-rectangle',
            'label': 'data(displayLabel)',
            'text-opacity': 1,
            'color': '#e2e8f0',
            'font-size': 10,
            'font-weight': '600',
            'text-outline-color': '#0b0f17',
            'text-outline-width': 2,
            'text-valign': 'center',
            'text-halign': 'right',
            'text-margin-x': 8,
            'text-max-width': '200px',
            'text-wrap': 'wrap',
            'min-zoomed-font-size': 7,
        } as any,
    },
    // Terminals — big, priority-coloured, always labelled
    {
        selector: 'node[isDestination = 1]',
        style: {
            'shape': 'round-rectangle',
            'background-opacity': 1,
            'border-width': 3,
            'border-color': '#ffffff',
            'z-index': 9999,
            'label': 'data(displayLabel)',
            'text-opacity': 1,
            'font-size': 13,
            'font-weight': '700',
            'color': '#ffffff',
            'text-outline-color': '#000',
            'text-outline-width': 3,
            'text-valign': 'bottom',
            'text-halign': 'center',
            'text-margin-y': 12,
            'text-max-width': '160px',
            'text-wrap': 'wrap',
            'min-zoomed-font-size': 0,
        } as any,
    },
    {
        selector: 'node[visibilityState = "hidden"]',
        style: { 'display': 'none' } as any,
    },
    {
        selector: ':selected',
        style: {
            'border-width': 5,
            'border-color': '#fff',
            'border-opacity': 0.9,
            'overlay-padding': 8,
            'overlay-opacity': 0.18,
        } as any,
    }
];

export const getBaseEdgeStyles = (): Stylesheet[] => [
    {
        selector: 'edge[visibilityState != "hidden"]',
        style: {
            'width': 'data(edgeWidth)',
            'line-color': 'data(linkColor)',
            'line-style': 'data(linkDash)',
            'opacity': 'data(edgeOpacity)',
            'curve-style': 'data(curveStyle)',
            'haystack-radius': 0,
        } as any,
    },
    {
        selector: 'edge[isLine = 1]',
        style: {
            'width': 'data(edgeWidth)',
            'line-color': 'data(lineColor)',
            'opacity': 0.85,
            'curve-style': 'haystack',
            'haystack-radius': 0,
            'z-index': 80,
        } as any,
    },
    {
        selector: 'edge[visibilityState = "hidden"]',
        style: { 'display': 'none' } as any,
    }
];

export const getCytoscapeStyles = (): Stylesheet[] => [
    ...getBaseNodeStyles(),
    ...getBaseEdgeStyles(),
];
