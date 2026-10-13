//! Port of `boost/polygon/voronoi_builder.hpp` (`voronoi_builder<int32>`): Fortune's sweepline
//! algorithm for points and non-intersecting segments.

use cppstd::rbtree::RbMap;
use cppstd::{pop_heap_by, push_heap_by, sort_by};

use crate::diagram::VoronoiDiagram;
use crate::predicates::{
    circle_formation_predicate, event_less_circles, event_less_site_circle, event_less_sites, is_vertical, is_vertical_pts, node_less,
    point_comparison,
};
use crate::structures::{BeachLineData, BeachLineKey, CircleEvent, Point2d, SiteEvent, SourceCategory, NIL};

type BeachLine = RbMap<BeachLineKey, BeachLineData>;

/// `detail::ordered_queue<event_type, event_comparison_type>`: a priority queue of circle events
/// stored in an arena so that beach line nodes can refer to (and deactivate) them.
#[derive(Default)]
struct CircleEventQueue {
    events: Vec<(CircleEvent, usize)>,
    heap: Vec<usize>,
}

impl CircleEventQueue {
    fn less(events: &[(CircleEvent, usize)]) -> impl Fn(&usize, &usize) -> bool + '_ {
        // comparison(it1, it2) = event_comparison_type(*it1, *it2) = predicate(rhs.first, lhs.first)
        move |a: &usize, b: &usize| event_less_circles(&events[*b].0, &events[*a].0)
    }

    fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    fn top(&self) -> usize {
        self.heap[0]
    }

    fn pop(&mut self) {
        let less = Self::less(&self.events);
        pop_heap_by(&mut self.heap, less);
        self.heap.pop();
    }

    fn push(&mut self, e: (CircleEvent, usize)) -> usize {
        self.events.push(e);
        let idx = self.events.len() - 1;
        self.heap.push(idx);
        let less = Self::less(&self.events);
        push_heap_by(&mut self.heap, less);
        idx
    }
}

#[derive(Default)]
pub struct VoronoiBuilder {
    site_events: Vec<SiteEvent>,
    site_event_iterator: usize,
    end_points: Vec<(Point2d, usize)>,
    circle_events: CircleEventQueue,
    beach_line: BeachLine,
    index: usize,
}

fn end_point_less(end1: &(Point2d, usize), end2: &(Point2d, usize)) -> bool {
    point_comparison(&end2.0, &end1.0)
}

impl VoronoiBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Each point creates a single site event.
    pub fn insert_point(&mut self, x: i32, y: i32) -> usize {
        let mut s = SiteEvent::from_point(Point2d::new(x, y));
        s.initial_index = self.index;
        s.set_source_category(SourceCategory::SinglePoint);
        self.site_events.push(s);
        let r = self.index;
        self.index += 1;
        r
    }

    /// Each segment creates three site events: its start point, its end point and the segment.
    pub fn insert_segment(&mut self, x1: i32, y1: i32, x2: i32, y2: i32) -> usize {
        // Set up start point site.
        let p1 = Point2d::new(x1, y1);
        let mut s = SiteEvent::from_point(p1);
        s.initial_index = self.index;
        s.set_source_category(SourceCategory::SegmentStartPoint);
        self.site_events.push(s);

        // Set up end point site.
        let p2 = Point2d::new(x2, y2);
        let mut s = SiteEvent::from_point(p2);
        s.initial_index = self.index;
        s.set_source_category(SourceCategory::SegmentEndPoint);
        self.site_events.push(s);

        // Set up segment site.
        let mut s;
        if point_comparison(&p1, &p2) {
            s = SiteEvent::from_points(p1, p2);
            s.set_source_category(SourceCategory::InitialSegment);
        } else {
            s = SiteEvent::from_points(p2, p1);
            s.set_source_category(SourceCategory::ReverseSegment);
        }
        s.initial_index = self.index;
        self.site_events.push(s);
        let r = self.index;
        self.index += 1;
        r
    }

    /// Run sweepline algorithm and fill output data structure.
    pub fn construct(&mut self, output: &mut VoronoiDiagram) {
        output.reserve(self.site_events.len());

        self.init_sites_queue();
        self.init_beach_line(output);

        while !self.circle_events.is_empty() || self.site_event_iterator != self.site_events.len() {
            if self.circle_events.is_empty() {
                self.process_site_event(output);
            } else if self.site_event_iterator == self.site_events.len() {
                self.process_circle_event(output);
            } else {
                let top = self.circle_events.top();
                if event_less_site_circle(&self.site_events[self.site_event_iterator], &self.circle_events.events[top].0) {
                    self.process_site_event(output);
                } else {
                    self.process_circle_event(output);
                }
            }
            while !self.circle_events.is_empty() && !self.circle_events.events[self.circle_events.top()].0.is_active {
                self.circle_events.pop();
            }
        }
        self.beach_line.clear();

        output.build();
    }

    pub fn clear(&mut self) {
        self.index = 0;
        self.site_events.clear();
    }

    fn init_sites_queue(&mut self) {
        sort_by(&mut self.site_events, |a, b| event_less_sites(a, b));
        // Remove duplicates (std::unique keeps the first of each run).
        let mut w = 0usize;
        for r in 0..self.site_events.len() {
            if w == 0 || !(self.site_events[w - 1] == self.site_events[r]) {
                self.site_events[w] = self.site_events[r];
                w += 1;
            }
        }
        self.site_events.truncate(w);
        for cur in 0..self.site_events.len() {
            self.site_events[cur].sorted_index = cur;
        }
        self.site_event_iterator = 0;
    }

    fn init_beach_line(&mut self, output: &mut VoronoiDiagram) {
        if self.site_events.is_empty() {
            return;
        }
        if self.site_events.len() == 1 {
            output.process_single_site(&self.site_events[0]);
            self.site_event_iterator += 1;
        } else {
            let mut skip = 0;
            while self.site_event_iterator != self.site_events.len()
                && is_vertical_pts(&self.site_events[self.site_event_iterator].point0, &self.site_events[0].point0)
                && is_vertical(&self.site_events[self.site_event_iterator])
            {
                self.site_event_iterator += 1;
                skip += 1;
            }

            if skip == 1 {
                self.init_beach_line_default(output);
            } else {
                self.init_beach_line_collinear_sites(output);
            }
        }
    }

    fn init_beach_line_default(&mut self, output: &mut VoronoiDiagram) {
        let first = self.site_events[0];
        let second = self.site_events[1];
        let end = self.beach_line.end();
        self.insert_new_arc(&first, &first, &second, end, output);
        self.site_event_iterator += 1;
    }

    fn init_beach_line_collinear_sites(&mut self, output: &mut VoronoiDiagram) {
        let mut it_first = 0usize;
        let mut it_second = 1usize;
        while it_second != self.site_event_iterator {
            let first = self.site_events[it_first];
            let second = self.site_events[it_second];
            let new_node = BeachLineKey::new(first, second);
            let edge = output.insert_new_edge(&first, &second).0;
            let end = self.beach_line.end();
            self.beach_line.insert_hint(end, new_node, BeachLineData::new(edge), node_less);
            it_first += 1;
            it_second += 1;
        }
    }

    fn deactivate_circle_event(&mut self, node: usize) {
        let ce = self.beach_line.value(node).circle_event;
        if ce != NIL {
            self.circle_events.events[ce].0.is_active = false;
            self.beach_line.value_mut(node).circle_event = NIL;
        }
    }

    fn process_site_event(&mut self, output: &mut VoronoiDiagram) {
        let mut site_event = self.site_events[self.site_event_iterator];

        let mut last = self.site_event_iterator + 1;

        if !site_event.is_segment() {
            while !self.end_points.is_empty() && self.end_points[0].0 == site_event.point0 {
                let b_it = self.end_points[0].1;
                pop_heap_by(&mut self.end_points, end_point_less);
                self.end_points.pop();
                self.beach_line.erase(b_it);
            }
        } else {
            while last != self.site_events.len()
                && self.site_events[last].is_segment()
                && self.site_events[last].point0 == site_event.point0
            {
                last += 1;
            }
        }

        // Find the node in the binary search tree with left arc lying above the new site point.
        let new_key = BeachLineKey::single(self.site_events[self.site_event_iterator]);
        let mut right_it = self.beach_line.lower_bound(&new_key, node_less);

        while self.site_event_iterator != last {
            site_event = self.site_events[self.site_event_iterator];
            let mut left_it = right_it;

            if right_it == self.beach_line.end() {
                // The above arc corresponds to the second arc of the last node.
                left_it = self.beach_line.prev(left_it);
                let site_arc = self.beach_line.key(left_it).right_site;
                right_it = self.insert_new_arc(&site_arc, &site_arc, &site_event, right_it, output);
                let lk = *self.beach_line.key(left_it);
                self.activate_circle_event(&lk.left_site, &lk.right_site, &site_event, right_it);
            } else if right_it == self.beach_line.begin() {
                // The above arc corresponds to the first site of the first node.
                let site_arc = self.beach_line.key(right_it).left_site;
                left_it = self.insert_new_arc(&site_arc, &site_arc, &site_event, right_it, output);
                if site_event.is_segment() {
                    site_event.inverse();
                }
                let rk = *self.beach_line.key(right_it);
                self.activate_circle_event(&site_event, &rk.left_site, &rk.right_site, right_it);
                right_it = left_it;
            } else {
                let site_arc2 = self.beach_line.key(right_it).left_site;
                let site3 = self.beach_line.key(right_it).right_site;

                // Remove the candidate circle from the event queue.
                self.deactivate_circle_event(right_it);

                left_it = self.beach_line.prev(left_it);
                let site_arc1 = self.beach_line.key(left_it).right_site;
                let site1 = self.beach_line.key(left_it).left_site;

                let new_node_it = self.insert_new_arc(&site_arc1, &site_arc2, &site_event, right_it, output);

                self.activate_circle_event(&site1, &site_arc1, &site_event, new_node_it);

                if site_event.is_segment() {
                    site_event.inverse();
                }
                self.activate_circle_event(&site_event, &site_arc2, &site3, right_it);
                right_it = new_node_it;
            }
            self.site_event_iterator += 1;
        }
    }

    fn process_circle_event(&mut self, output: &mut VoronoiDiagram) {
        let top = self.circle_events.top();
        let (circle_event, bisector_node) = self.circle_events.events[top];
        let mut it_first = bisector_node;
        let mut it_last = it_first;

        // Get the C site.
        let mut site3 = self.beach_line.key(it_first).right_site;

        // Get the half-edge corresponding to the second bisector - (B, C).
        let bisector2 = self.beach_line.value(it_first).edge;

        // Get the half-edge corresponding to the first bisector - (A, B).
        it_first = self.beach_line.prev(it_first);
        let bisector1 = self.beach_line.value(it_first).edge;

        // Get the A site.
        let site1 = self.beach_line.key(it_first).left_site;

        if !site1.is_segment() && site3.is_segment() && site3.point1 == site1.point0 {
            site3.inverse();
        }

        // Change the (A, B) bisector node to the (A, C) bisector node.
        self.beach_line.key_mut(it_first).right_site = site3;

        // Insert the new bisector into the beach line.
        let new_edge = output.insert_new_edge_circle(&site1, &site3, &circle_event, bisector1, bisector2).0;
        self.beach_line.value_mut(it_first).edge = new_edge;

        // Remove the (B, C) bisector node from the beach line.
        self.beach_line.erase(it_last);
        it_last = it_first;

        // Pop the topmost circle event from the event queue.
        self.circle_events.pop();

        // Check new triplets formed by the neighboring arcs to the left for potential circle events.
        if it_first != self.beach_line.begin() {
            self.deactivate_circle_event(it_first);
            it_first = self.beach_line.prev(it_first);
            let site_l1 = self.beach_line.key(it_first).left_site;
            self.activate_circle_event(&site_l1, &site1, &site3, it_last);
        }

        // Check the new triplet formed by the neighboring arcs to the right for potential circle events.
        it_last = self.beach_line.next(it_last);
        if it_last != self.beach_line.end() {
            self.deactivate_circle_event(it_last);
            let site_r1 = self.beach_line.key(it_last).right_site;
            self.activate_circle_event(&site1, &site3, &site_r1, it_last);
        }
    }

    fn insert_new_arc(
        &mut self,
        site_arc1: &SiteEvent,
        site_arc2: &SiteEvent,
        site_event: &SiteEvent,
        mut position: usize,
        output: &mut VoronoiDiagram,
    ) -> usize {
        // Create two new bisectors with opposite directions.
        let new_left_node = BeachLineKey::new(*site_arc1, *site_event);
        let mut new_right_node = BeachLineKey::new(*site_event, *site_arc2);

        // Set correct orientation for the first site of the second node.
        if site_event.is_segment() {
            new_right_node.left_site.inverse();
        }

        // Update the output.
        let edges = output.insert_new_edge(site_arc2, site_event);

        position = self.beach_line.insert_hint(position, new_right_node, BeachLineData::new(edges.1), node_less);

        if site_event.is_segment() {
            // Update the beach line with temporary bisector, that will disappear after processing
            // site event corresponding to the second endpoint of the segment site.
            let mut new_node = BeachLineKey::new(*site_event, *site_event);
            new_node.right_site.inverse();
            position = self.beach_line.insert_hint(position, new_node, BeachLineData::new(NIL), node_less);

            // Update the data structure that holds temporary bisectors.
            self.end_points.push((site_event.point1, position));
            push_heap_by(&mut self.end_points, end_point_less);
        }

        position = self.beach_line.insert_hint(position, new_left_node, BeachLineData::new(edges.0), node_less);

        position
    }

    fn activate_circle_event(&mut self, site1: &SiteEvent, site2: &SiteEvent, site3: &SiteEvent, bisector_node: usize) {
        let mut c_event = CircleEvent::default();
        // Check if the three input sites create a circle event.
        if circle_formation_predicate(site1, site2, site3, &mut c_event) {
            // Add the new circle event to the circle events queue. Update bisector's circle event
            // iterator to point to the new circle event in the circle event queue.
            let e = self.circle_events.push((c_event, bisector_node));
            self.beach_line.value_mut(bisector_node).circle_event = e;
        }
    }
}
