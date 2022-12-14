
use alloc::collections::BTreeMap;
use super::{VirtPageNum, address::VPNRange};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RangesChanged {
    Inserted(VPNRange),
    Removed(VPNRange),
    Extended(VPNRange,VPNRange),
    Shrinked(VPNRange,VPNRange),
    Combined(VPNRange,VPNRange),
    Splitted(VPNRange,VPNRange),
}

pub struct RangeManager {
    areas: BTreeMap<VirtPageNum,VirtPageNum>,
    // pub mmap_area: Option<MapArea>
}

impl RangeManager {
    pub fn new() -> Self {
        RangeManager { 
            areas: BTreeMap::new(), 
        }
    }
    pub fn insert(&mut self, rng:&VPNRange) -> Option<RangesChanged> {
        let in_between_range_last = self.areas.range(rng.get_start().. rng.get_end()).last();
        let next_range = self.areas.range(rng.get_end()..).next();
        
        let mut lower_merge = false;
        let mut low_range: Option<VPNRange> = None;
        //[start                                             end)
        //          [lower                    higher)
        match in_between_range_last {
            None => {   }
            Some((&higher,&lower)) => {
                //                             [start                    end)
                //          [lower                      higher)
                // if higher> rng.get_end() {   return None  } 
                if lower< rng.get_start() && rng.get_end() < higher{return None}
                if higher < rng.get_end() && higher!=rng.get_start() {return None}
                //                             [start                    end)
                //          [lower       higher)xxxxxxxxxxxxxxxxxxxxxxxxxxxxx
                else {
                    // 并发问题
                    // self.areas.remove(&higher);
                    // self.areas.insert(rng.get_end(),lower);
                    
                    lower_merge = true;
                    low_range = Some(VPNRange::new(lower,higher));
                }
            }
        }

        // let mut insert_directly = false;
        match next_range {
            Some((&higher,&lower)) => {
                //[start          end)
                //          [lower            higher)
                if lower < rng.get_end(){   return None;  } 
                //[start          end)
                //                   [lower            higher)
                else if lower==rng.get_end(){
                    if lower_merge {
                        self.areas.remove(&low_range.unwrap().get_end());
                        if let Some(old_lower_bound) = self.areas.get_mut(&higher) {
                            *old_lower_bound = low_range.unwrap().get_start();
                        }
                        return Some(RangesChanged::Combined( 
                            low_range.unwrap(),
                            VPNRange::new(lower,higher))
                        );
                    } else {
                        if let Some(old_lower_bound) = self.areas.get_mut(&higher) {
                            *old_lower_bound = rng.get_start();
                        }
                        return Some(RangesChanged::Extended( 
                            VPNRange::new(lower,higher),
                            VPNRange::new(rng.get_start(),higher))
                        );
                    }
                } else {
                    // insert_directly = true;
                }
            },
            None => {  
                // insert_directly = true;
            }
        }
        //                 [start          end)
        //         [       )                                  [lower            higher)
        // if lower_merge && insert_directly {
        if lower_merge {
            self.areas.remove(&low_range.unwrap().get_end());
            self.areas.insert(rng.get_end(),low_range.unwrap().get_start());
            return Some(RangesChanged::Extended( 
                low_range.unwrap(),
                VPNRange::new(low_range.unwrap().get_start(),rng.get_end())))
        }
        // else if insert_directly {
        else {
            self.areas.insert(rng.get_end(),rng.get_start());
            return Some(RangesChanged::Inserted( 
                VPNRange::new(rng.get_start(),rng.get_end()))
            );
        }
        // return None
    }
   
    pub fn remove(&mut self,rng:&VPNRange) -> Option<RangesChanged>{
        //                   [start          end)
        //           [?lower        [?lower           [?lower            higher)
        match self.areas.range(rng.get_end()..).next() {
            None => return None,
            Some((&higher,&lower)) => {
                //                   [start          end)
                //                           [lower     higher)
                if rng.get_start()< lower {return None}
                //                   [start          end)
                //                   [lower       higher)
                if lower == rng.get_start()&& rng.get_end()== higher {
                    self.areas.remove(&higher);
                    return Some(RangesChanged::Removed( 
                        VPNRange::new(lower,higher))
                    );
                }
                //                         [start          end)
                //             [lower                   higher)
                //             [lower start)
                if lower < rng.get_start() && rng.get_end()== higher  {
                    self.areas.remove(&higher);
                    self.areas.insert(rng.get_start(), lower);
                    return Some(RangesChanged::Shrinked( 
                        VPNRange::new(lower,higher),
                        VPNRange::new(lower, rng.get_start()))
                    );
                }
                //             [start          end)
                //             [lower                   higher)
                //                                [end  higher)
                if lower == rng.get_start()&& rng.get_end()< higher {
                    if let Some(old_lower_bound) = self.areas.get_mut(&higher) {
                        *old_lower_bound = rng.get_end();
                    }
                    return Some(RangesChanged::Shrinked( 
                        VPNRange::new(lower,higher),
                        VPNRange::new(rng.get_end(), higher))
                    );
                }
                //               [start          end)
                //     [lower                             higher)
                if lower < rng.get_start()&& rng.get_end()< higher {
                    if let Some(old_lower_bound) = self.areas.get_mut(&higher) {
                        *old_lower_bound = rng.get_end();
                    }
                    self.areas.insert(rng.get_start(), lower);
                    return Some(RangesChanged::Splitted( 
                        // VPNRange::new(lower,higher),
                        VPNRange::new(lower, rng.get_start()),
                        VPNRange::new(rng.get_end(), higher))
                    );
                }
            }
        }
        return None
    }
}


// #[cfg(test)]
// mod tests {

//     #[test]
//     fn test_range_manager() {
//         let mut manager = RangeManager::new();
//         manager.insert(&VPNRange::new(0x3.into(),0x7.into()));
//         manager.insert(&VPNRange::new(0x20.into(),0x40.into()));
//         manager.insert(&VPNRange::new(0x10.into(),0x14.into()));

//         println!("{:?}",manager.areas);
//         // 需要实现PartialEq, Eq以及Debug才能与None比较
//         assert_eq!(manager.insert(&VPNRange::new(0x9.into(),0x12.into())), None);
//         assert_eq!(manager.insert(&VPNRange::new(0x9.into(),0x15.into())), None);
//         assert_eq!(manager.insert(&VPNRange::new(0x11.into(),0x12.into())), None);
//         assert_eq!(manager.insert(&VPNRange::new(0x11.into(),0x15.into())), None);
//         assert_eq!(manager.insert(&VPNRange::new(0x5.into(),0x15.into())), None);

//         let res = manager.insert(&VPNRange::new(0x0.into(),0x2.into()));
//         assert_eq!(res.unwrap() , RangesChanged::Inserted(
//             VPNRange::new(0x0.into(),0x2.into()))
//         );
//         let res = manager.insert(&VPNRange::new(0x50.into(),0x60.into()));
//         assert_eq!(res.unwrap() , RangesChanged::Inserted(
//             VPNRange::new(0x50.into(),0x60.into()))
//         );
//         let res = manager.insert(&VPNRange::new(0x40.into(),0x41.into()));
//         assert_eq!(res.unwrap() , RangesChanged::Extended(
//             VPNRange::new(0x20.into(),0x40.into()),
//             VPNRange::new(0x20.into(),0x41.into()))
//         );
//         let res = manager.insert(&VPNRange::new(0x18.into(),0x20.into()));
//         assert_eq!(res.unwrap() , RangesChanged::Extended(
//             VPNRange::new(0x20.into(),0x41.into()),
//             VPNRange::new(0x18.into(),0x41.into()))
//         );
//         let res = manager.insert(&VPNRange::new(0x41.into(),0x50.into()));
//         assert_eq!(res.unwrap() , RangesChanged::Combined(
//             VPNRange::new(0x18.into(),0x41.into()),
//             VPNRange::new(0x50.into(),0x60.into()),
//         )
//         );

//         println!("{:?}",manager.areas);

//         assert_eq!(manager.remove(&VPNRange::new(0x16.into(),0x17.into())), None);
//         assert_eq!(manager.remove(&VPNRange::new(0x16.into(),0x20.into())), None);
//         assert_eq!(manager.remove(&VPNRange::new(0x50.into(),0x90.into())), None);
//         assert_eq!(manager.remove(&VPNRange::new(0x80.into(),0x90.into())), None);
//         assert_eq!(manager.remove(&VPNRange::new(0x16.into(),0x90.into())), None);

//         let res = manager.remove(&VPNRange::new(0x3.into(),0x7.into()));
//         assert_eq!(res.unwrap() , RangesChanged::Removed(
//             VPNRange::new(0x3.into(),0x7.into()))
//         );

//         let res = manager.remove(&VPNRange::new(0x54.into(),0x60.into()));
//         assert_eq!(res.unwrap() , RangesChanged::Shrinked(
//             VPNRange::new(0x18.into(),0x60.into()),
//             VPNRange::new(0x18.into(),0x54.into()))
//         );
//         let res = manager.remove(&VPNRange::new(0x18.into(),0x23.into()));
//         assert_eq!(res.unwrap() , RangesChanged::Shrinked(
//             VPNRange::new(0x18.into(),0x54.into()),
//             VPNRange::new(0x23.into(),0x54.into())
//         ));
//         let res = manager.remove(&VPNRange::new(0x30.into(),0x40.into()));
//         assert_eq!(res.unwrap() , RangesChanged::Splitted(
//             VPNRange::new(0x23.into(),0x30.into()),
//             VPNRange::new(0x40.into(),0x54.into()),
//         ));

//         println!("{:?}",manager.areas);

//     }
// }